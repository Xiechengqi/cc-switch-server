# Claude OAuth 单账号反代

本文描述 cc-switch-server 对 Claude OAuth 的生产边界、运行时约束和验收方式。目标是把一个明确绑定的 Claude OAuth 账号稳定暴露为 Anthropic Messages API，不提供账号池、负载均衡或跨账号故障转移。

## 能力边界

- 对外入口为 Router 暴露的 Share URL 下 `POST /v1/messages` 及对应的 `count_tokens` 路径。
- Share 精确绑定一个已启用的 Claude Surface；OAuth Provider Bundle 必须绑定一个明确账号。
- 同一个 Share URL 也可承载该 Bundle 已启用的 Codex/Gemini Surface。
- 不按并发、配额、健康度或错误类型切换到其他 Claude Provider 或账号。
- 可在同一 Provider、同一账号上执行文档中列出的有限重放。

这条边界用于避免一次生成请求被两个账号同时执行、重复计费、上下文漂移，以及刷新 token 在账号之间串用。

## 入口鉴权

客户端只连接 Router Share URL，并使用 Router 用户令牌。Router 转发到 Server 时附加签名 ingress context；Server 必须同时验证 Router 身份和非空 Share 身份。

- 未签名请求返回 `401`。
- 签名有效但没有 Share 身份的 client-lane 请求返回 `403`。
- 客户端提供的 `Authorization`、`x-api-key`、Share header 或 Provider header 都不能替代签名 context，也不能改写 binding。
- `15721` 不签发或接受本地推理 token，不提供 `/r/:key` Provider 入口；该端口只承载管理 UI、控制面、健康检查和 Router 内部 ingress。

Router ingress v2 使用 `cc-switch-router-ingress-v2` HMAC 域，并把 method、完整 path/query、实际发送 body 的 SHA-256 和每次请求唯一的 request ID 一起签入 context。Server 先验证 envelope HMAC 和 Router/installation 绑定，再按普通请求 2 MiB、媒体 32 MiB、Codex Images 48 MiB 的上限读取 body，随后验证 request binding 并登记最多 16,384 项的 replay cache；method/path/body 篡改或同 installation 下 request ID 重放均返回 `401`。replay cache 满载且没有过期项可清理时拒绝新请求，不驱逐仍在有效期内的记录。v1 只兼容到 `2026-09-08T00:00:00Z`（含边界时刻），之后 fail closed。

## Provider 与账号固定

Share 请求的选择过程如下：

1. 从已验证 ingress context 解析唯一 Share，并读取其 Claude binding。
2. 编译后的 RuntimePlan 必须与该 Surface 的已提交 revision 一致。
3. 解析 Provider Bundle 的不可变账号绑定。
4. 检查账号登录状态、配额/冷却状态和并发上限。
5. 必要时先完成该账号的 token refresh，再获取该账号的 in-flight lease 开始转发。

Share 不存在、没有 Claude binding、Surface 已禁用、账号需要重新登录、账号处于冷却或并发已满时，请求直接失败。系统不会查找第二个 Claude Provider。

首次上游 `401` 需要强制刷新时，当前 inference lease 会先释放；刷新成功后只为同一 Provider、同一账号重新获取 lease 并重放一次。这样 refresh 网络等待不占用生成并发槽，也不会借刷新机会切换身份。

Gemini 的首次 project discovery 使用独立 operation lock，不复用 token refresh 锁，也不在网络探测期间占用账号 inference lease。该隔离保证跨协议共用账号基础设施时不会因无关控制面操作阻塞 Claude 请求。

## OAuth 刷新一致性

Claude OAuth refresh token 可能在刷新成功时立即轮换，旧 token 随即失效。因此刷新提交遵循以下顺序：

1. 在账号单飞锁内调用 token endpoint。
2. 校验响应与账号身份，构造候选 AccountStore。
3. 原子写入完整 `accounts.json` 候选快照。
4. 写入成功后发布内存快照。
5. 如果写入失败，仍发布新的旋转 token，进入 persistence degraded，并返回 `503`。
6. 后台以 1 秒起步、最大 60 秒的指数退避重试完整账号快照。

降级状态使用带失败代次的原子状态。旧的成功重试不能清除更新一代的落盘失败，避免错误地恢复 readiness 后在进程重启时丢失最新 refresh token。

刷新 token 与 profile/quota enrichment 分离。profile 请求失败不会污染已经成功的 token 轮换提交。

OAuth endpoint fallback 只用于明确的 connect-stage 网络失败（请求尚未送达 token endpoint）或 endpoint unavailable 状态（如 404、405、410、501、502、503、504）。请求送出后的响应头/响应体读取失败不会 fallback，避免在服务端已轮换 refresh token 时用旧 token 再请求备用端点。`invalid_grant`、access denied、429 和其他确定性拒绝不会尝试第二个 token endpoint。`Retry-After-Ms`、秒格式 `Retry-After` 和 HTTP-date 均会被解析并限制在 24 小时内。

所有生产 token rotation 路径，包括请求前预刷新、`401` 强制刷新、后台 warm refresh 和控制面手动刷新，都进入同一个 detached owner 协调器。等待同一次 refresh 的请求被取消时不会取消真正持有 token rotation 的 owner。owner 有独立的 30 秒 deadline；panic、超时以及 token endpoint 已可能接收请求但 Server 未得到完整 receipt 的情况都收敛为 unknown outcome，立即隔离旧 refresh token 并要求重新登录，不会盲目重试。panic 恢复提交使用 refresh generation CAS，旧 owner 不能覆盖更新一代的成功 token 或失败隔离。进程收到退出信号后先停止接受新的 managed refresh，并最多等待 35 秒排空既有 owner。

## Wire profile、控制面与模型目录

Claude OAuth wire 身份固定在脱敏合同 `assets/contract/claude-oauth-wire-profile.json`，当前 profile 为 `claude-code-2.1.280-audited-2026-09-23`：Claude Code `2.1.280`、Stainless `0.112.1`、Node `v26.3.0` 和 Axios `1.15.2`。该版本来自 2026-09-23 官方 npm latest Linux x64 native binary 的静态审计，冻结 binary SHA-256；审计没有使用真实凭据或访问 Anthropic，合同没有保存 token、账号标识、原始请求正文、定价或 provider fallback，真实订阅账号验证仍为 pending。usage 使用 `claude-code/2.1.280`，bootstrap/inference 使用 CLI identity，profile、roles 和 token endpoint 使用 Axios identity。默认身份可用 `CC_SWITCH_CLI_UA_VERSION` 或完整 `CC_SWITCH_CLI_UA` 紧急覆盖；候选版本必须不低于内建 profile，合法完整 UA 优先，旧值或无效值会被拒绝并 fail closed 到固定 profile。启动日志与 profile gauge 会显示最终版本、来源和是否拒绝了旧覆盖。

`node scripts/audit/audit-claude-wire-profile.mjs` 默认离线校验合同、Rust 常量和 `.env.example` 的一致性；scheduled/人工升级审计可显式运行 `node scripts/audit/audit-claude-wire-profile.mjs --check-npm` 对比 npm `latest` dist-tag并记录 `stable`。Server 运行时不会访问 npm，也不会因审计网络不可用影响反代。`.env.example` 故意将两个 identity override 留空，避免旧部署配置永久压过更新后的内建 profile。

2.1.280 billing 的 `cc_version` 使用 `<公开版本>.<三位 prompt fingerprint>`。fingerprint 在任何 system→message 迁移前从原始请求第一条真实 user text 取样，按 JavaScript UTF-16 code unit 索引 4、7、20（缺位补 `0`），拼接 salt `59cf53e54c78` 和有效 CLI version 后计算 SHA-256，取前三位 hex；`ping` 的 2.1.280 golden 是 `d7b`，历史 2.1.258 golden `1e2` 继续保留用于算法回归。billing block 不带 `cache_control`，后续 identity/prompt block继续承担缓存断点。CCH 仍按官方 `cch=00000` 占位行为生成，seed 为 `0x4D659218E32A3268`；签名在日期、工具、thinking、cache-control 等 body rewrite 全部完成后计算，所有 `model` 字符串递归置空，并递归排除 `max_tokens`、`fallbacks`、`fallback_credit_token`。2.1.280 golden CCH 为 `daec4`，历史 2.1.258 golden `8d393` 继续保留。紧急回滚可设置 `CC_SWITCH_CLAUDE_CCH_POLICY=disabled`，此时 billing block 中的 CCH 成员会被移除，不会回退到旧 seed。

quota refresh 并行获取 usage、profile、bootstrap 和 `/api/oauth/claude_cli/roles`。四路 enrichment 都有超时和有界响应读取；roles/profile/bootstrap 失败只影响辅助证据，不覆盖已经成功的 token 或 usage quota，也不在日志、指标或公开 API 中暴露原始 OAuth body。

Claude Max 倍率按 usage、bootstrap、profile 和 canonical cache 的固定权威顺序解析。实时 `default_claude_max_5x` / `default_claude_max_20x` 会发布 `planType`、`planLabel`、`planSource`、`planStale` 和 `planObservedAt`。缓存倍率只可在同一 Max family 内细化实时 generic `Claude Max`，最多复用 24 小时；缓存回填继承原 `planObservedAt`，不会用本次 `queriedAt` 续命。超龄倍率退出候选，存在实时 generic Max 时立即降级显示为 `Claude Max`；quota 成功但实时响应没有任何有效计划证据、兼容缓存也已超龄时，refresh 会显式清除账号持久化的旧 `subscriptionLevel`，避免账号列表永久残留 5x/20x 标签。UI 对仍在有效窗口内的缓存计划显示缓存标识，真实 5x/20x smoke gate 则只接受非 stale 证据。

Claude OAuth 模型发现不调用 Anthropic，也不会为了列模型刷新 token。管理 discovery 和 Share `GET /v1/models?app=claude` 都使用同一份版本化静态目录：

- `claude-opus-4-6`
- `claude-sonnet-4-6`
- `claude-haiku-4-5-20251001`
- `claude-fable-5-1`
- `claude-fable-5`
- `claude-opus-5`
- `claude-opus-4-8`
- `claude-sonnet-5`
- `claude-opus-5-5`

Share 响应同时公开 `source=claude_code_wire_profile`、`stale=false` 和 capture 的 `fetchedAtMs`。更新 Claude Code wire profile 时必须一起 review capture、Rust 常量、beta 矩阵、endpoint identity 和模型目录，不能只改 User-Agent 字符串。

## 请求分类、beta 与缓存

客户端分类采用 fail-closed 多信号：精确 Claude CLI User-Agent、`x-app=cli`、metadata/session 关系和 beta/body profile 必须共同成立，才进入 Native CLI、SDK CLI、VSCode 或 helper 最小修改路径。UA 或 billing 单信号不能伪装成 native；不确定请求按第三方 Anthropic 客户端处理。确认的 native/helper 请求不会被补入 `tools: []`、默认 `max_tokens`、thinking 或 identity block，只保留 credential-scoped OAuth/extended-cache beta 与最终 CCH。可用 `CC_SWITCH_CLAUDE_NATIVE_PASSTHROUGH=disabled` 强制回到第三方规范化路径。

确认的 native 请求若在历史开头带 system/developer 文本 turn，会把可提升的文本块移到顶层 `system`，但同一原消息上的 `output_config` 等消息级字段及非文本内容块必须留在原位置；纯文本且无其他字段的原消息才可删除。混合消息拆分在最终 CCH 前完成，避免努力档位或未来扩展块因文本提升而静默丢失。该本地保真规则不表示 Anthropic 已验证接受所有未来块形状；未获支持的形状仍由上游判定。

Claude OAuth 出站 Messages/count_tokens 会在最终 CCH 之前，仅对历史 `tool_use.id` 与 `tool_result.tool_use_id` 的非空、非 `[A-Za-z0-9_-]` 字符串建立请求内确定性别名；相同原 ID 的调用和结果共用别名，已有合法 ID 保持原字节且占用其名字空间，别名碰撞顺延到未使用值。其他 `id` 字段及空值不据此改写。该局部修复针对推荐参考的 Claude 工具 ID 约束，不把外部项目的 Devin 编码/还原协议迁入本 Server；真实 OAuth 接受性仍待 receipt。

Messages beta 由确定顺序的 capability engine 生成，未知客户端 beta 不透传。2.1.280 新增的 per-turn control/timing、mid-conversation tool changes、inline tools、system clear-at、dangerous tool safeguards、thinking binding/resumption 和 prompt cache eviction 均按模型能力、正文形状或显式请求 gate；Opus 5.5 自动启用 per-turn control，timing 只在顶层/消息级 `output_config.timing` 或显式请求时启用。Opus 5.5 的 OpenAI ingress reasoning 转换也读取同一 capability registry：显式 disabled/none 会收敛为 adaptive/low，避免发送该模型拒绝的 `thinking.type=disabled`。`thinking-display-updates-2026-08-18` 只在 `thinking.display="updates"` 时加入并与 redact beta 互斥。普通工具数组不触发 advanced-tool-use；只有审计过的 regex/BM25 tool-search 类型或 `defer_loading=true`（含 `custom.defer_loading`）才加入。Advisor 只识别精确的 `tools[].type=advisor_20260301`。普通 OAuth Messages 不再无条件获得 fallback-credit；只有显式 beta、有效 `fallback_credit_token` 或已获准保留的 OAuth `fallbacks` 才加入。服务端不会主动开启 model fallback：数组和 `default` 分别要求客户端显式请求匹配的 server-side-fallback beta；错配和 `count_tokens` fallback 字段均在最终 CCH 前移除。`count_tokens` 使用独立小集合，不继承 generation-only beta。模型目录、发现 metadata、reasoning 转换和 beta engine 共用 versioned capability registry；未知模型不乐观注入 model-gated beta。可用 `CC_SWITCH_CLAUDE_BETA_PROFILE=minimal` 回退到最小服务端 profile，不改变账号绑定。

cache-control pass 在 CCH 前全局扫描 tools、system 和 messages，规范化 TTL/order、最多保留 4 个 breakpoint，并优先保留最后 tool、最后 system 和最近 message marker；重复执行幂等。forced `tool_choice` 会同步移除不兼容的 thinking/context-management。可用 `CC_SWITCH_CLAUDE_CACHE_REWRITE=disabled` 关闭该 pass。

第三方请求的默认 tool 缓存断点只落在最后一个非 deferred tool；`defer_loading=true` 与 `custom.defer_loading=true` 都不接收自动注入。若全部工具都 deferred，则不注入 tool 断点，仍可在最近 user message 注入滚动断点。客户端显式提供的 tool 缓存标记不会因本规则被改写或删除。当前 Responses `tool_search` 仍经兼容式 `tool_search` 函数桥，不等同于上游原生 tool-search；显式 `execution=server` 等非客户端执行的声明与历史调用/结果会在 Responses→Anthropic 转换前返回 400，不能伪装成客户端可执行函数。缺省 execution 的既有兼容形状和显式 `client` 保持。原生开关、模型/credential 能力与请求内固定决策须另行差分和真实接受性验证后才能开放。

请求类别 cache matrix 进一步区分 main、subagent、probe/title helper、native helper 与 `count_tokens`：main native 保留 extended profile；subagent 默认回到 5m，但 body/header 明确请求 1h 时保留；probe/helper 总是抑制 1h；`count_tokens` 不发 generation-only extended TTL。判断只扫描受支持的 cache-control 位置，不把历史正文中的任意同名字段当成 TTL 请求。

OpenAI Chat/Responses 转 Claude 时，只有 `response_format=json_object|json_schema` 或 Responses `text.format` 会加入 Claude-scoped structured-output system instruction，并保留已有 operator instructions；普通 text 请求不改写。OpenAI Chat 流转 Anthropic 时，tool/text/reasoning block 采用严格单开块状态机，交错内容与后续 tool 在当前 tool 结束后按序释放。Claude 流转 OpenAI Chat 时，usage 从 `message_start` 与多个 `message_delta` 按字段合并；原始下游明确 `stream_options.include_usage=true` 才在 finish 后、`[DONE]` 前得到唯一 `choices: []` usage tail，显式零和 cache read/write 均保留。失败前局部 usage 仍由 server-side accumulator 写入失败记录，不重复计费。

Codex Responses 入口转到 Claude OAuth 时，省略或设为 `null` 的输出上限先按目标 Claude 模型目录填充，再计算 reasoning budget：已注册的非 Fable 模型使用目录中的 `max_output_tokens`，Fable 默认保守限制为 64,000；未知模型沿用共享转换器的 8,192。显式提供的 `max_output_tokens`、兼容别名 `max_completion_tokens` 或 `max_tokens` 保持原有转换语义；此规则不扩展到 Chat、Claude API Key 或其他 Provider。模型目录上限是静态能力证据，不代表当前订阅账号已获真实接受性验证。

helper request ID 遵循真实上游 base：`api.anthropic.com` 缺值时生成 UUID v4；自定义 base 只转发 confirmed helper 已携带的 `x-client-request-id`，不合成 first-party 指纹。上述差异由 `assets/contract/claude-reference-delta.json` 与 `scripts/audit/audit-claude-reference-delta.mjs` 定期审计；外部 checkout 仅是可选只读证据，真实账号验收继续为 `live_pending`。

## 凭据形态与可选隐私增强

Claude 凭据显式区分 `refreshable_oauth` 与 `access_only_setup_token`。OAuth exchange 只有在 scope 明确包含 `user:inference` 且不含 profile/office scope 时，才允许缺失 refresh token；该 scope 会在任何 bootstrap/profile 请求之前识别并直接跳过无权访问的 enrichment。手工 credentials import 则由 access-only 凭据拓扑标记 setup-token。setup-token 不进入 refresh，profile/quota 403 只降低 enrichment，不阻断 inference；缺失账号 UUID 时使用带域分离的 credential digest 生成不可逆内部 ID。过期后直接要求重新导入，不形成 refresh loop。可用 `CC_SWITCH_CLAUDE_SETUP_TOKEN=disabled` 关闭 access-only exchange/import 入口，标准 refreshable OAuth 不受影响。

日期指纹规范化仍是默认关闭的证据门控隐私增强；工具名 alias 则承担默认开启的协议兼容职责：

- `CC_SWITCH_CLAUDE_DATELINE_NORMALIZATION=enabled` 只规范 system text 和 `<system-reminder>` 内已验证的日期指纹，不修改普通用户正文、assistant 正文或 tool input/result。
- `CC_SWITCH_CLAUDE_CUSTOM_TOOL_ALIAS` 默认开启，但只 alias 不符合 `^[A-Za-z0-9_-]{1,64}$` 的普通 custom/MCP 名称；合法自定义名称逐字节保留，Claude Code 内建名称继续使用既有 canonical case，server tool 原样保留。映射仅存在于当前请求，declaration、forced choice、历史 `tool_use` / `tool_reference`（含 `tool_result.content[]` 与 `tool_search_tool_result.content.tool_references[]` 中的协议引用）及 JSON/SSE 响应共用同一可逆 map；普通文本和工具 `input` 不递归改写。最多 128 项，大小写或 wire alias 碰撞直接拒绝。事故回滚可设为 `disabled`，此时非法名称会保持原样且由上游决定是否接受。

显式 session 的 Claude Messages `thread` 续接还可在进程内短时保存上一条成功响应的工具别名。缓存按 Share、用户、session、Provider revision/runtime、Account ID/实例/代次及实际出站 Bearer 与账号凭据摘要隔离；同 ID 重建账号或同代次直接更换 token 不会读取旧别名。缺少必需历史状态时发网前返回 404：即使本轮声明了新工具，只要历史 `tool_use`、协议 `tool_reference`、`tool_choice` 或工具增删引用了未声明且需要别名／内建工具大小写归一的名称，仍须取得旧状态；这也包括 `tool_addition.tool_definition.definition.name` 的非 server 内联定义，server 工具不因此被拦截。完整声明及无需改名的合法名称保持原路径。顶层和 `tool_result`/`tool_search_tool_result` 中的协议 `tool_reference` 共用别名恢复，普通文本与工具 `input` 保持不透明。检测在构建临时声明集合前限制至 128 项，同原有 alias 上限一致。同一精确作用域的上游 message ID 若被不同别名映射重复使用，缓存会标记冲突直到原 TTL 到期；后续续接即使带完整工具声明，也在发网前返回 409，而不在两个映射间任选其一。此缓存不承诺跨进程恢复，也不能代替真实 Anthropic 续线程验收。

没有上游拒绝证据，因此 TLS ClientHello/header-order 模拟仍不实现；当前继续使用 rustls transport。

## 响应头容量窗口

Anthropic 的主动 `/api/oauth/usage` 响应不保证包含 Fable 独立周窗口。Server 因此在 managed Claude OAuth 的 Messages 共用响应头边界采样固定白名单：`anthropic-ratelimit-unified-{5h,7d,7d_oi}-{utilization,reset,status}`。该入口位于 JSON、SSE 和 429 分支之前；同一个上游响应只采样一次，采样或持久化失败不会改变代理响应。

- utilization 只接受有限数值 `0..1.05`，轻微超限归一为 `1`；负数、NaN、无穷和明显异常值丢弃。精确 `status=rejected` 且没有 utilization 时可合成 `100%`。
- reset 支持相对秒、epoch 秒/毫秒、RFC3339 和 HTTP-date；5h 与周窗口分别限制在 6 小时和 8 天的合理未来范围。缺 reset 的样本最多保留 1 小时。
- 样本绑定请求实际使用的 Account ID 和 `authIdentityGeneration`，按窗口单调、部分合并；旧响应、旧身份和过期样本不能覆盖当前状态。token-only refresh 保留样本，重新登录/主体变化清空样本。
- 主动 usage 中的同名 tier 始终优先；被动样本只补缺，不覆盖主动值。投影使用最新贡献样本更新 `queriedAt`。
- `7d_oi` 映射为真实数值 tier `seven_day_fable`，并公开 `scope=model_family`、`capacityPool=claude_fable_7d_oi`、`modelFamily=claude-fable-5`、`relativeWeeklyCapacity=0.5` 与 `source=anthropic_ratelimit_7d_oi`。新鲜、无冲突且明确为 Max 5x/20x 的计划在尚无真实 `7d_oi` 样本时，只通过独立 `unobservedTiers` 投影 `Fable 7d · 待观测`；它没有 utilization/reset，不得伪装为 `0%`，也不参与调度、限流、可用性或容量评分。真实样本到达后数值 tier 覆盖提示；样本到期而计划仍新鲜时回退到待观测。stale/generic Max 不生成提示，只有成功 Fable 请求或明确 Fable-only 429 的直接证据才可展示真实 tier；明确 Free/Pro/Team/Enterprise 永不展示。

普通样本保存在独立 quota observation 中，不写入代表“已耗尽并阻断”的 `capacity_pool_limits`。UI 继续复用 generation-aware `oauth-quota-updated`；首次出现、tier/reset 变化和跨入 100% 立即通知，普通百分比变化最多 30 秒通知一次。reset/TTL 到期会移除样本并通知 UI/Share 投影。Server 不会为了填充 Fable 数值主动发送推理或探测请求；数值只能来自真实 usage/响应头观测。所有行为只针对一个明确绑定账号，不引入账号池、选号或 fallback。

## 429 与响应编码

Claude 429 不再默认升级为账号 cooldown：明确 unified 5h/7d `rejected` 会通过 AccountStore CAS 写当前账号 cooldown；只有 Fable 请求同时满足 `7d_oi=rejected` 且 5h/7d 均为 `allowed` 或 `allowed_warning`，才归为 Fable-only，并写入只阻断 Fable family 的账号 `claude_fable_7d_oi` capacity-pool marker；存在 Share 时同时写当前 Share+model cooldown。它不会阻断同账号的 Sonnet/Opus 等共享窗口模型。`unified=rejected` 或 `7d_oi=rejected` 但共享窗口缺失/不明确时保守归为账号 cooldown。普通 `Retry-After` 和未知 429 只写当前 Share+model cooldown；Fast credits/entitlement refusal 不写任何 cooldown。解析支持 RFC3339、epoch seconds、epoch milliseconds 和 HTTP-date，并对异常未来时间应用全局上限。429 不 replay、不换账号、不换 Provider、不换模型。

统一响应解码支持 gzip/x-gzip、deflate、brotli 和 zstd，支持逗号分隔或重复 `Content-Encoding` 的最多 4 层堆叠；无 header 时只对强 gzip/zstd magic 做 sniff。每层和最终 body 均受累计大小限制。成功、错误、SSE 和 `count_tokens` 共用该治理入口；解码后失效的 `Content-Encoding`/`Content-Length` 不会继续透传。普通无压缩 SSE 只预读足够判断 magic 的最多 4 字节后继续增量转发，因此仍保留首事件、idle timeout 和客户端取消语义；只有明确压缩或强 magic 命中的 SSE 才在 64 MiB 上限内缓冲解码，截断、未知编码或解压超限返回不含上游正文的 `502`，不会透明重放。可用 `CC_SWITCH_RESPONSE_EXTENDED_DECODING=disabled` 关闭 br/zstd/magic 扩展，gzip/x-gzip/deflate 的既有显式编码解码仍保留。

高风险行为按 wire profile 独立回滚：CCH、beta profile、native passthrough、cache rewrite、扩展响应解码、setup-token、dateline 和 tool alias 均有单独开关。Claude 429 scope classifier 不提供会重新启用“普通 429 写账号 cooldown”的 legacy 开关，因为该旧行为会在单账号场景错误阻断其他模型；必要时只能回滚完整版本。TLS/header-order 模拟因没有可重复拒绝证据而未实现，也没有伪造一个无效开关。

## 重放矩阵

| 场景 | Messages 生成 | count_tokens | Provider/账号 |
| --- | --- | --- | --- |
| 建连失败 | 最多重放 1 次 | 有界重放 | 原 Provider、原账号 |
| 401 且账号支持刷新 | 强制刷新后重放 1 次 | 强制刷新后重放 1 次 | 原 Provider、原账号 |
| OAuth compatibility 400 / signature 错误 | 有界 body 兼容重放 | 不适用 | 原 Provider、原账号 |
| header/首事件超时 | 不重放 | 有界重放 | 不跨账号 |
| 响应 body 读取失败 | 不重放 | 有界重放 | 不跨账号 |
| 429 / 529 | 不重放 | 不重放 | 原 Provider、原账号 |
| 已向下游提交 SSE 事件后的中断 | 不重放，发送终止错误帧 | 不适用 | 不跨账号 |

Messages 的建连重放只发生在请求尚未到达可产生计费副作用的阶段。任何不明确的发送或读取错误均按“可能已执行”处理。

## Anthropic 语义守卫

原生 Anthropic 响应在返回客户端前进行协议检查：

- 非流式 Messages 必须是 `type=message`，包含非空 `id` 与 `model`、assistant role、content 数组和非负 usage；SSE `message_start.message` 使用同一字段校验，缺失或空白 `model` 在成功提交前拒绝。
- 非流式 Messages 的已知 content block 还须保持载荷形状：`text.text` 与 `thinking.thinking` 为字符串（可为空），`tool_use`/`server_tool_use` 有非空 `id`、`name` 和对象型 `input`；无类型或非对象块拒绝，未来未知类型仍按兼容路径保留。流式工具块继续由起始事件与增量生命周期检查，不将完整非流块规则套在尚未结束的流式分片上。
- `count_tokens` 必须包含非负整数 `input_tokens`。
- SSE 按字节增量解析，支持任意 chunk 边界、UTF-8、LF/CRLF/CR 及混合空行分隔、comment 和多行 `data`；Claude OAuth 工具名响应恢复共用相同事件边界。
- 流必须包含一个 `message_start`；成功 `message_stop` 前还须至少有一条经校验的 `message_delta`（usage 含非负 `output_tokens`）。业务事件只能出现在 start 之后，Anthropic `error` 可直接结束失败流；仅有 start/stop 的截断流不得记为成功。
- 已知内容块增量在成功提交前还要核对载荷类型：`text_delta.text`、`thinking_delta.thinking`、`signature_delta.signature` 为字符串，`citations_delta.citation` 为对象；空字符串允许，未知未来内容块类型仍按原兼容策略处理。
- `redacted_thinking` 是独立的不透明内容块：SSE 起始块和非流式 Messages 内容都必须携带非空字符串 `data`，SSE 块不得再接普通思考 delta；代理不解析或改写有效 `data`。
- Messages JSON 与 SSE `message_delta.delta` 的显式 `stop_reason` 必须为非空字符串或 `null`，`stop_sequence` 必须为字符串或 `null`；SSE 已宣布非空终止原因后不得再出现内容块，后续同名终止原因和独立 usage 更新可保留。
- 空流、半帧、缺少终止帧、终止后的数据、事件名与 `data.type` 不一致及超过 8 MiB 的单事件都会失败。
- 8 MiB 限额按单条完整事件或尚未完成的当前事件计算；同一网络块中先有完整事件、后有超限半帧时立即拒绝半帧，多条各自合法的事件不因网络块总长度超限而误拒绝。
- 下游已经收到事件后发生协议截断时，代理补发 Anthropic 终止错误帧，不透明重放整个请求。
- 收到 `message_stop` 后代理停止读取上游，及时释放连接和账号 lease。

客户端主动断开会取消上游 body，usage 日志记录为 `client_cancelled`，但不会把 Provider 标记为网络故障。

## 可观测性

- `GET /health` 始终反映进程存活；降级时 `status=degraded`。
- `GET /ready` 在 OAuth credential persistence degraded 时返回 `503`。
- `cc_switch_credential_persistence_degraded`：落盘降级 gauge。
- `cc_switch_stream_client_cancelled_total{app="claude"}`：客户端取消计数。
- `cc_switch_claude_retry_total`：按 stage/source 分类的 Claude 重放。
- `cc_switch_proxy_semantic_guard_total`：Anthropic 文档和流语义观察结果。
- `cc_switch_oauth_refresh_attempt_total` / `cc_switch_oauth_refresh_unknown_outcome_total`：有界 Provider/outcome 分类的刷新结果。
- `cc_switch_account_lease_total`：有界 Provider/result 分类的 lease 获取结果。
- `cc_switch_account_inflight{provider_type}`：按有限 Provider 类型聚合的当前账号请求数。
- `cc_switch_provider_outcome_total{app,provider_type,outcome}`：按有限 app、Provider 类型和结果聚合的上游终态。
- `cc_switch_claude_roles_total`、`cc_switch_claude_ttfb_seconds`、`cc_switch_claude_stream_duration_seconds`、`cc_switch_claude_semantic_failure_total`：roles enrichment 和流语义时延/终态。
- `cc_switch_claude_wire_profile_info`：固定 wire profile、effective Claude Code version、身份来源与有界 `stale_override_rejected` 标签。
- `cc_switch_claude_client_class_total`：按有界客户端分类和请求类型计数。
- `cc_switch_claude_rate_limit_scope_total`：按 request、Share+model、account unified 作用域计数。
- `cc_switch_claude_quota_header_observation_total{outcome}`：Messages 响应头 quota 样本的 absent/unmanaged/updated/unchanged/stale_identity 有界结果，不记录账号或 header 原值。
- `cc_switch_claude_optional_rewrite_total`：只记录有界 rewrite 类型，不记录日期或工具名。
- `cc_switch_claude_response_decoding_total{surface,result}`：按 `json`、`error`、`sse`、`count_tokens` 与有界解码结果计数，不记录编码正文或客户端值。

Prometheus 标签禁止包含账号 ID、Provider ID 或 request ID；这些实例标识只留在受访问控制且已脱敏的诊断数据中，不能进入长期时序基数。

`/ready` 失败应阻止新实例进入负载，但不强制终止当前进程；内存中的新 token 仍可用于完成已有流量和后台落盘恢复。

## 真实账号验收

真实验收分为 `oauth_inference`、`max_5x_plan`、`max_20x_plan` 和 `fable_5_1` 四个互相独立的 operation。`claude-oauth-real.mjs` 仍可先做低成本 probe，但 probe 输出不能提升 live 状态：

```bash
SERVER_URL='https://server.example.com' \
CC_SWITCH_SERVER_TOKEN='<server-session-token>' \
CC_SWITCH_SHARE_URL='https://share.example.com' \
ROUTER_API_TOKEN='<router-user-token>' \
CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT='<account-id-or-email>' \
CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT='<account-id-or-email>' \
node scripts/smoke/claude-oauth-real.mjs
```

可选变量：

- `CC_SWITCH_CLAUDE_MODEL`：覆盖默认模型 `claude-sonnet-4-6`。
- `CC_SWITCH_REAL_TIMEOUT_MS`：单请求超时，范围 1 秒到 5 分钟。
- `ROUTER_API_TOKEN_HEADER`：Router 使用非默认鉴权 header 时覆盖，默认 `Authorization`。

Share probe 通过同一个 Share URL 检查 count_tokens、非流式 Messages 和完整 SSE lifecycle。两个 Max 变量分别按 Claude OAuth 账号 ID 或 email 精确匹配 `GET /api/accounts`，再调用 `GET /api/accounts/:id/quota?refresh=true&force=true`，检查账号/配额显示名、canonical `planType` / `planLabel` 以及 source/stale/conflict evidence 的一致性。脚本不输出账号选择器、完整 email或失败响应正文。

最终验收必须为每个 operation 单独准备仓库外、权限 `0600` 的私有 receipt，并设置对应的 `CC_SWITCH_CLAUDE_*_PROVIDER_ID`、`*_SHARE_ID`、`*_MODEL` 和 `*_REAL_RECEIPT_FILE`：

```bash
RUN_REAL=1 \
CC_SWITCH_CLAUDE_REAL_OPERATION='oauth_inference' \
SERVER_URL='https://server.example.com' \
CC_SWITCH_SERVER_TOKEN='<server-session-token>' \
CC_SWITCH_SHARE_URL='https://share.example.com' \
ROUTER_API_TOKEN='<router-user-token>' \
CLAUDE_OAUTH_TEST_ACCOUNT='<account-id-or-email>' \
CC_SWITCH_CLAUDE_OAUTH_PROVIDER_ID='<provider-id>' \
CC_SWITCH_CLAUDE_OAUTH_SHARE_ID='<share-id>' \
CC_SWITCH_CLAUDE_OAUTH_MODEL='claude-sonnet-4-6' \
CLAUDE_OAUTH_INFERENCE_REAL_RECEIPT_FILE='/private/claude-oauth-inference.json' \
node scripts/smoke/claude-real-receipt.mjs --operation oauth_inference
```

其他 operation 使用 `.env.example` 中各自的 Max 5x、Max 20x、Fable 5.1 和 Opus 5.5 变量。validator 先核对当前 Git commit、唯一 Claude OAuth Account generation、`oauth.claude_messages` Provider、明确 Share binding 和精确模型；计划 operation 还会强制刷新并绑定 fresh canonical plan projection。`oauth_inference` 因要求同账号首次 401 恢复，Max 5x/20x 与 Fable 5.1 因要求套餐证据，均只接受控制面明确投影为 `refreshable_oauth` 且持有 refresh token 的 Account；验收开始和结束都复核，缺能力字段或凭据丢失即失败。Opus 5.5 没有上述恢复/套餐检查，仍可用 `access_only_setup_token` 单独验收，不能把前四项的凭据门槛无条件套给它。Fable 5.1 必须使用精确 `claude-fable-5-1` 和 Max 20x 计划，不能由普通模型或 Max 20x 计划解析结果替代。

Share 控制面投影的 `bindings` 已包含 `share.app` 主条目，不把主字段再当作额外绑定。五项 validator 均要求 Claude App 在原始 `bindings` 中恰好一条匹配所选 Provider，并核对主字段与其对应的唯一条目一致；Share 还须明确 `enabled=true/status=active`。重复、冲突或停用时在模型目录请求前失败关闭。此项只约束验收证据，不改变生产 Share 路由，也不使控制面多次读取成为原子快照。

五项 validator 还要求控制面按真实 `ProviderView` 明确给出顶层 `revision` 与 runtime `providerRevision`：两者均为非负安全整数且相等；Share `configRevision` 也必须明确为非负安全整数，runtime `runtimeFingerprint` 必须是非空白字符串。旧 mock 的虚构顶层 `providerRevision`、缺失或冲突的真实两份 revision 不能补齐 receipt 作用域；字段不完整时在模型目录请求前拒绝。此检查只保证本地证据作用域的字段完整性，不证明厂商接受性或跨 API 原子快照。

五项 receipt 的 `recordedAt` 均须在最终接受时仍处于 24 小时窗口；Max 5x、Max 20x、Fable 5.1 的 canonical plan `observedAt` 还须处于 15 分钟窗口。validator 在末次 Account/Provider/Share 复核后，以同一时刻再次检查两类证据，期间过期即拒绝。Opus 5.5 和普通 OAuth inference 不会被套上套餐时间门槛。这只是私有验收时效规则，不改变生产 Claude wire，也不是跨 API 原子快照。

私有 receipt 在任何控制面请求前由只读描述符限量读取；读取前后还比较同一文件的 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime。普通等长原位改写即使发生在首次读取后、EOF 检查前，也会失败关闭。这是文件快照稳定性门槛，不保证元数据可复原的并发对手面前具有原子视图，也不证明部署 commit 与本地 HEAD 一致。

最终接受前还会重新解析配置的 receipt 路径，并与读取时保存的文件指纹比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime。若控制面校验期间路径换成另一文件，或原文件被等长原位改写并留下元数据变化，验收失败关闭。这不是跨 API 与文件系统的原子事务；恶意元数据复原及最终检查后的变化仍需由受控私有文件位置和运行环境防护。

最终文件复核还会通过 `O_NOFOLLOW` 只读描述符重新读取至多 1 MiB 的完整 receipt 原始字节及 EOF，并与最初验收的字节逐一比较；读前、读后及配置路径的文件身份也继续复核。五个 operation 的对抗 fixture 在控制面请求期间等长改写文件、让末次元数据读取返回旧值时，旧流程会错误接受，现在全部脱敏拒绝。元数据伪装只用于验证内容不能由元数据替代，并不表示普通用户能恢复真实 `ctime`；末次检查后的并发变化及跨 API 原子性仍不由此保证，离线 fixture 也不能提升真实 Anthropic 接受性。

五项模型目录验收都以 `app=claude`、`expectedShareId` 与 `expectedProviderId` 请求签名 Router ingress 的 `/v1/models`。Server 先按实际签名 Share 选择 Provider，再拒绝与所选 operation 预期不符的目录；普通 `providerId` 查询参数不承担选择作用。此门槛只证明目录路由绑定，静态 Claude 模型目录命中不等于订阅 entitlement，也不使目录与后续请求成为原子快照。

五项私有 validator 还要求初次目录明确为 `source=claude_code_wire_profile`、`stale=false`，且 `fetchedAtMs` 为有效的正整数 capture 时间；末次 Account/Provider/Share 绑定复核后，经同一签名 Share/Provider 栅栏再读取目录。两次完整 `data` 的所有模型 ID 均须为非空字符串、无首尾空白且唯一，选定模型各恰好出现一次；目录按 ID 确定性排序后计算摘要，因此纯行重排可接受，能力字段等内容变化仍拒绝。两次来源、stale、capture 时间与摘要须相同；模型增删或元数据漂移会拒绝 receipt。静态 capture 时间不套用动态目录 TTL；本检查只保证一次验收内的目录稳定性，目录摘要不写入现有 schema-v1 receipt，也不能证明真实 Anthropic entitlement 或跨 API 原子性。

四个主体 operation 加 Opus 5.5 扩展分别判断输入并维持五份独立 receipt。输入不全时 validator 输出 `blocked_inputs/live_pending`；fixture 模式只能验证合同并输出 `contract_verified/live_pending`。任何一个 operation 的通过都不能提升其他项，也绝不能把 SKIP、fixture 或 probe 结果记为真实通过。

## 非目标与剩余外部风险

- 不实现多账号调度、权重、健康故障转移或 quota spillover。
- 不保证上游账号本身的订阅状态、风控状态或区域可用性。
- 本地测试不能证明真实 Anthropic OAuth endpoint、真实模型权限和长时间流在生产网络中可用；这些只能由上面的 external gate 给出证据。
- persistence degraded 期间若宿主机在重试成功前崩溃，旋转后的 token 仍可能丢失；readiness 和告警用于缩短这一风险窗口，无法替代可靠磁盘。

## 流式 message_delta 的结构边界（2026-10-10）

Claude OAuth 流只有在 `message_start`、至少一条含对象 `delta` 且 usage 合法的 `message_delta`、`message_stop` 满足既有顺序时才可记为完成。缺 `delta` 或其值为 `null`、字符串、数组均在检查阶段失败；空对象 `{}` 仍兼容已有 usage-only 形状。此处仅加固本地成功门槛，不改变出站请求或推断真实厂商支持范围；五项 operation receipt 与续线程/beta 验收仍待独立证据。

## SSE 本地协议错误的回显边界（2026-10-10）

流式检查器拒绝事件名与 `data.type` 不一致、未知事件先于 `message_start`、内容块 delta 类型不兼容等畸形流时，本地 502 诊断不回显上游提供的未知事件名或类型。已知协议类型仍用于定位结构错误；正常上游 `error` 事件按原语义处理。此项只防止本地协议诊断成为上游文本回显通道，不修改 Claude OAuth 出站 wire 或五项真实验收状态。

## SSE 检查器失败状态锁存（2026-10-10）

一次 `push` 或 `finish` 已因协议或容量错误失败后，同一 `AnthropicSseInspector` 不得再被迟到事件修复为成功；成功终态后又发现冲突帧，也不得让 `finish` 洗白该错误。第一次错误保留原分类，后续调用返回固定本地错误且不再暴露成功终态或 message ID。正常消息、上游 `error` 事件和出站 Claude OAuth 请求不变。该离线状态机约束不构成真实订阅账号验收，五项 operation receipt 与续线程/beta 仍待证。

## 终止原因的类型与后续内容栅栏（2026-10-10）

流式 `message_delta.delta.stop_reason` 显式为非空字符串时，检查器记住其值；随后任何 `content_block_start/delta/stop` 都作为本地协议错误拒绝，第二个不同终止原因也拒绝。缺省或 `null` 不打开此栅栏，保留 usage-only delta 后继续内容的既有兼容；`stop_sequence` 只校验字符串或 `null`，不据离线 fixture 限定厂商停止原因枚举。非流式 Messages JSON 共用字段类型校验。Claude OAuth HTTP 回环确认迟到内容不能记为成功或 completed 用量；此项不改变出站请求、CL-N7/CL-N8 或五项真实 receipt 的 `live_pending` 状态。
