# Kiro Prompt Cache 本地计量合同

状态：`KI-01`、`KI-02`、`KI-03` 与 CORE-N2 为 `fixture_verified`；CORE-N1 facade、八路私有 receipt gate 与 EVID-N1 schema v2 已完成；`KI-04`、`KI-05` 仍为 `live_pending`，共享/远端缓存运行时关闭。

## 边界

该缓存只在 Kiro 没有返回权威 token usage 时模拟 Anthropic Prompt Caching 的下游计量字段。它不会改变发给 Kiro 的请求，不证明 Kiro 复用了推理前缀，也不降低或重算上游账单。每次调用仍固定在已解析的 Provider、Share、Account、签名用户与 credential generation；缓存 miss、持久化失败和重启都不能触发账号池、轮换、credential rail fallback、跨账号或跨 Provider fallback。

缓存 namespace 由调用链在进入本模块前固定，使用无歧义 JSON framing 覆盖 App、Provider id/revision、runtime fingerprint、Account id、持久化 `accountOwnerId`、`auth_identity_generation`、`token_refresh_generation`、请求身份摘要、Share、签名用户、route、runtime region 和 session。请求身份以域分离 SHA-256 绑定规范化出站 Bearer、refresh token、实际机器 ID、profile ARN、区域、endpoint、认证方式及 token type 等，不在键中保留凭据原文；同代次换凭据不复用旧本地计量。owner ID 随 Kiro/Amazon Q 账号首次创建而随机生成，正常更新与重启保留，删除重建或切换 ProviderType 后重新生成。因此相同身份在重启后的 TTL 内仍可命中，而同 ID、同代际、同凭据的在线删除重建不会复用旧估算。旧 `accounts.json` 缺 owner ID 时按 ProviderType 与 Account ID 派生稳定迁移值，首次保存后持久化；仅凭完全相同的旧文件，无法识别离线手工重建这一历史事件。直接调用、未绑定 AccountStore 的测试路径使用独立 `direct-unbound` 作用域，不具备生产绑定语义。Claude 与 Codex 的 session precedence 继续由 `assets/contract/kiro-wire-protocol.json` 定义；缺 session 时使用请求级随机作用域，不能形成跨请求匿名命中。model、tool choice、thinking 和 output config 还会进入前缀哈希，防止不同推理合同复用同一个估算条目。

## KI-01：断点与 TTL

本地语义同时支持顶层 `cache_control` auto-caching 与 tool/system/message content 的显式断点。两类断点统一排序、去重并共享最多四个槽位。顶层 auto 在多轮请求中优先落到最新 user input 之前的最后一个可缓存 block，确保最新提问保持 uncached；只有首轮没有更早可缓存内容时才落到当前 user block。

前缀签名只剔除每个 tool/system/message block 顶层用于声明断点的 `cache_control`；工具 schema 或内容内部同名字段属于请求内容，仍参与哈希。签名域已升为 v4，旧 v3 条目不会因历史递归剔除而与新请求碰撞；已有本地估算缓存会自然冷启动。此规则只影响本地计量估算，不改变出站请求或证明上游实际 cache hit。

全局上下文中的 `model`、`tool_choice`、`thinking`、`output_config` 会递归按对象键排序后进入签名；纯 JSON 键顺序变化不会使相同合同的本地估算意外 miss，字段值变化仍产生独立签名。数组顺序与实际值保持原样，不改发往 Kiro/Amazon Q 的 body。

每个断点向前最多检查 20 个 lookback position，只能命中此前真实写入过的断点。连续 `tool_use` block 合为一个 position，连续 `tool_result` block 也合为一个 position。thinking/redacted thinking 与空 text 不能作为断点。`ephemeral` 缺 TTL 或 `5m` 使用 300 秒，`1h` 使用 3600 秒；命中按条目自己保存的 TTL 滑动续期。混合 TTL 只允许 1h 断点位于 5m 断点之前。

非法 type/TTL、不可缓存断点、冲突的 message/block 声明、超过四个断点和反向混合 TTL 都整次禁用本地缓存估算，不截断声明或制造部分命中。`cc_switch_kiro_prompt_cache_decisions_total{decision}` 使用固定低基数原因记录 auto、explicit、no-cache 与 fail-closed 结果。

## KI-02：权威 usage 与比例守恒

本地缓存先在自身 estimator 口径计算三项：完整 prompt、最深断点覆盖前缀和最深已命中前缀。拿到 Kiro 的 `metricsEvent`/`contextUsageEvent` total 后，以纯整数、四舍五入方式先把覆盖比例映射到权威 total，再把命中比例映射到覆盖部分；剩余量确定性归入 uncached input。因此三项始终非负，并严格满足：

`input_tokens + cache_creation_input_tokens + cache_read_input_tokens == authoritative total`

只要上游 `messageMetadataEvent.tokenUsage` 存在，或 metrics 明确出现任一 cache usage 字段，上游值就优先，包括显式 0；缺失的另一个 cache bucket 视为 0，绝不以本地命中覆盖上游零值。响应 usage 用 `cache_usage_source=upstream_token_usage` 或 `local_prompt_cache_estimate` 公开来源；本地路径还给出固定低基数 `cache_usage_decision`。

对象型 `metadataEvent.tokenUsage` 是最终快照：输入分项全部缺失或合计为 0 时，0 仍覆盖此前的 `contextUsageEvent`/`metricsEvent` 估算；之后到达的回退事件不得覆盖这份快照。同一条流重复下发快照时取最后一份，不累加；后一份缺失的输出或缓存分项也按零处理，不能沿用前一份。这个优先级不把尚未验证的 context 百分比减去本轮输出，KI-N5 的该分摊仍只作 shadow 对照。

权威 `tokenUsage` 的显式计数字段（含已支持的 camel/snake 别名）必须是 `i32` 范围内的 JSON 整数；小数、字符串、`null` 字段值和越界数在 EventStream 解析时失败关闭，不能截断或伪装成零快照。整体 `tokenUsage:null` 与缺失字段继续按无快照/默认零的原有规则处理，合法负整数仍在最终用量中归零。此规则不改变 `metricsEvent` 回退或 KI-N5 shadow 的权威性。

## KI-03：SQLite 持久化

请求线程只更新有界内存表和 dirty mutation map，并用容量为 2 的 non-blocking 通知队列唤醒专用 writer。writer 在请求路径之外合并更新，失败时按 25ms 到 2s 有界退避；响应不会等待磁盘，也不会因写失败声称 durable。指标分别记录 coalesced、queue full、write success/failure、startup load 和 shutdown flush，`cc_switch_kiro_prompt_cache_persistence_degraded` 明确当前 durability 状态。

配置 `CC_SWITCH_KIRO_PROMPT_CACHE_PATH=/path/base` 时，新存储位于 `/path/base.sqlite`。旧 JSON 只在 SQLite 空库时读取一次，导入后保留原文件作为回滚输入，不再同步重写。SQLite 使用 WAL、`synchronous=FULL`、transactional upsert/delete、每条 mutation generation 与全局 generation CAS；过期清理和 4096 条容量限制在同一事务中完成。并发更新在内存中按 key/generation 合并，失败批次只在没有更新版本时回填。进程优雅退出在 HTTP drain 后执行有界 shutdown flush。

本地验收覆盖 SQLite transaction 冲突回滚、不可写路径降级、后台写不阻塞请求、restart TTL 命中、断点/TTL/lookback 语义和 usage 守恒。它不替代断电时的文件系统保证，也不把缓存可用性提升为请求成功条件。

## KI-04：真实验收

真实 receipt 必须按 auth kind 与 runtime region 分开，覆盖 `builder_id`、`idc`、`social`、`api_key` × `us-east-1`、`eu-central-1`。每份 receipt 固定当前 target commit、一个 Account auth/token generation、Claude/Codex 两个 Provider revision/runtime、一个 Share revision、签名用户/session、exact model 和两份 fresh catalog；执行 CountTokens 零推理发网、Claude/Codex non-stream/stream、tool namespace、auto/explicit/no-cache、5m/1h、四断点、20-block 边界、最新 user、上游 usage 优先、401/timeout/throttle/代际漂移、Compact 零发网、decoy 和 secret scan。OAuth kind 只允许同账号首次 401 pre-commit 重放，API Key 401 直接终止。

`scripts/smoke/kiro-real-receipt.mjs` 以 `--auth-kind` 与 `--region` 逐份验证仓库外 `0600` 私有 receipt；还要求公开 Account 投影中的 `kiroCredentialScope.authKind/runtimeRegion` 与所选作用域一致。该投影使用当前认证方式和实际运行区域解析，不公开 profile ARN、原始 profile 或凭据；旧服务缺字段时验收失败关闭。`scripts/audit/kiro-real-receipt.test.mjs` 在 fixture 模式覆盖全部八组及漂移/伪造负例。fixture 只可得到 `contract_verified/live_pending`，不能生成真实通过。当前八份合同 receipt 仍全部为 `null`，全部保持 `live_pending`。

八个 scope 的私有 validator 都直接核对 Share 原始 `bindings`：Claude、Codex 各须恰好一条精确 Kiro Provider 绑定，Share 主字段须与对应的唯一主绑定一致，并要求 `enabled=true/status=active`。重复、冲突或停用的 Share 在两份目录请求前拒绝；主字段不会被合成为额外 binding。控制面的分次读取不构成跨 API 原子快照。

两份 Provider 均按真实控制面投影验证：顶层 `revision` 与 runtime `providerRevision` 必须明确提供非负安全整数且相等，runtime `runtimeFingerprint` 必须是非空白字符串；Share `configRevision` 也必须明确提供非负安全整数。任何一侧缺失、冲突、仅存在旧 mock 所用的顶层 `providerRevision` 或空白 fingerprint，都会在 Claude/Codex 目录请求前拒绝，不能以 `0` 回退补齐 receipt scope。这仍只是私有证据门槛，不构成跨 API 原子快照或真实厂商接受性。

八个 scope 还要求 Account 投影的 `needsRelogin` 明确为布尔 `false`。字段缺失、`null` 或字符串 `"false"` 均在 Provider 与两份模型目录请求前失败关闭；这只约束私有验收证据，不改变 Builder ID、IdC、Social 或 API Key 的生产认证路由。

每份验收会分别向同一 Router Share 请求 Claude、Codex 两份目录，并在各自请求附上可选 `expectedShareId/expectedProviderId`。Server 将其与签名 Share 和该 App 实际选定的 Provider 核对；仅传普通 `providerId` 查询不能证明目录来自 receipt 所声明的绑定。此检查不替代 Account auth kind/runtime region 投影，也不把目录存在视作厂商接受性。

两份目录各自须在接受结束时仍满足五分钟新鲜窗口、最多一分钟的未来时钟偏差。validator 在末次 Account/双 Provider/Share 复核之后，经同一签名 Share 分别重读 Claude、Codex 目录：每份完整 `data` 的所有模型 ID 必须为非空、无首尾空白且唯一，所选模型也须恰好一次；目录来源受限且非 stale。完整 `data` 按模型 ID 排序后计算摘要，末次摘要须与 receipt 绑定的初次摘要相同；纯行顺序变化不算内容漂移，能力字段等内容变化仍会拒绝。两次目录各自的时间戳均在共同接受时刻检查。`kiro_list_available_models` 与 `kiro_account_cache` 之间的合法来源切换不要求字符串相同。取目录时尚未过期但复核期间过期，或目录内容/权威改变，都不能接受该 receipt。这是私有证据门槛，不更改生产目录 TTL，也不是跨 API/上游原子快照。

八份 receipt 的 `recordedAt` 也须在最终接受时仍处于 24 小时窗口。内容校验和末次绑定复核后分别检查，最终与两份目录使用同一接受时刻；任一证据在复核期间过期都拒绝该 auth-kind×region scope。此规则不改变 receipt schema v1/revision 1、KI-N5 计量权威或 Compact/shared cache 的禁用状态。

私有 receipt 在任何控制面请求前由同一只读描述符限量读取。读取前后核对 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；普通同 inode 原位改写若改变这些元数据，会在发网前脱敏拒绝。元数据复原或精确并发竞态下的原子文件快照未由此证明，也不构成真实 Kiro 接受性。

最终接受前还会重新解析配置的 receipt 路径，将当前文件的 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime 与读取后的指纹比较。控制面校验期间若换成另一文件，或原文件被等长原位改写并留下元数据变化，该 auth-kind×region scope 失败关闭。末次检查不是跨 API/文件系统原子事务；恶意元数据复原及检查后的变化仍需由受控私有文件位置和运行环境防护。

最终复核也会经 `O_NOFOLLOW` 只读描述符重读至多 1 MiB 的完整 receipt 原始字节和 EOF，并与初次快照比较；读前、读后和配置路径的文件指纹仍须一致。八个 auth-kind×region scope 的对抗 fixture 都在控制面请求期间等长改写文件，并用测试预加载钩子让末次元数据读取返回旧指纹；旧逻辑错误接受，新增字节复核后均脱敏拒绝。此钩子仅说明元数据不能替代内容校验，不代表普通用户可以恢复真实 `ctime`；末次检查后的竞态和跨 API 原子性仍不保证，fixture 也不是厂商真实接受性。

生产模型目录缓存还按进程内 Account incarnation 与出站请求身份隔离。除 App、Provider revision/runtime、账号代际、profile ARN 和区域外，缓存键包含 access token、认证方式、token 类型与机器 ID 的域分离 SHA-256 摘要，不保存这些原文；同 ID/同代际的凭据替换、同 token 的账号删除重建、机器身份变化都不能直接命中旧目录。控制面在刷新后和返回前复核账号实例、请求身份、有效运行身份与 Provider 绑定，漂移则不发布本次目录。此栅栏不改变五分钟 fresh/二十四小时 stale TTL、出站 Kiro wire 或八份真实 receipt 状态；完全相同的身份在 fresh TTL 内遇到上游权益变化，仍遵循原有缓存时效。分次读取仍非跨存储原子事务。

## KI-05：共享缓存门禁

共享/远端缓存当前 `runtimeEnabled=false`。只有明确的多副本需求、本地 SQLite 不足的运行证据、固定 namespace/version、TLS/secret 管理、短 timeout、熔断和本地降级设计全部完成后，才能另行评审。参考项目的 Redis、singleflight、session affinity、账号调度和命中驱动路由均未迁入；任何未来 remote hit 也不得改变固定 Account binding。

## CORE-N1：Provider lifecycle facade

`src/proxy/providers/kiro/` 现在承载 Kiro/Amazon Q 产品类型、canonical request/model/session、Account-bound catalog/region/request preparation、本地 CountTokens、keepalive、错误分类和 subscription throttle 决策。AWS EventStream/协议转换继续位于 `src/proxy/kiro.rs`；Share/Account lease、usage、terminal、统一 attempt budget 和同账号 401 replay 所有权继续位于共享 forwarder。该拆分不引入账号选择、跨产品共享 credential、额外 attempt 或 wire 变化。

Kiro 请求构造只将 user 内容数组中的直系 image，以及其中 tool_result 内容数组的直系 image，放入 Kiro user message 的 `images`。图片预处理先检查显式图片是否位于这些可承载位置；system、assistant、对象型 user 内容或对象型 tool_result 内容中的图片固定、脱敏地拒绝，不会处理完图片又发出缺图请求。受支持图片继续执行原有 base64、格式、尺寸、去重及内存预算检查；此项不增加新的 Kiro wire 图片位置。

EventStream 的上游 `error`/`exception` 帧可带不可信 header 错误码和 payload 文本；它们可能回显请求信息。下游流与非流错误只公开本地 `KIRO_UPSTREAM_STREAM_ERROR` 和通用消息，不透传这两个上游诊断字段。结构损坏、容量限制等由本地生成的 EventStream 错误仍保留原有分类；该展示边界不改变请求身份、解码、恢复或计量合同。

`toolUseEvent` 的工具 ID、名称和无效 JSON 也由上游提供，不能在失败响应中当作可信诊断输出。`TOOL_JSON_INVALID` 与 `TOOL_JSON_INCOMPLETE` 继续保留分类；对客户端只给固定说明及本地统计的缓冲字节数，不显示工具 ID、名称或解析器详情。有效工具调用仍按原值转发；本项不改变工具 JSON 的聚合与恢复规则。

当前请求若声明了多个同名子工具，上游只返回短名称时仍拒绝歧义映射，分类为 `KIRO_EVENT_STREAM_INVALID`。错误说明现在固定，不回显上游短名称；精确声明名和唯一子名称仍按原规则恢复。非流与流式路径共用该拒绝，流式失败不会发成功 `message_stop`。这仅改变诊断展示，不放宽工具归属或八路账号/区域绑定。

工具 `input_schema` 的顶层 `oneOf`/`anyOf`/`allOf` 仅在恰好一个显式 object 分支时沿用兼容展开；多个分支、无显式对象、畸形组合值或与根级属性冲突时，在出站前固定返回 400，避免静默丢失合法参数形状。Claude Code 内建工具的字段映射也递归进入该单一分支，使发往 Kiro 的 schema 与工具输入同名；`Read.pages` 的原有禁止规则同样覆盖此分支。此限制不改变嵌套属性的组合语义，也不以离线校验推断厂商接受性。

EventStream 帧中的未知事件名、重复 header 名与无效 payload 的解析器说明同样属于不可信诊断。客户端仍收到 `KIRO_EVENT_STREAM_INVALID`，但消息只保留固定的“payload 无效”或“重复 header”说明，不回显这些上游字符串；内部 `WireError` 保留原始字段供受控调试。流式与非流式转换使用同一脱敏边界，合法事件和 wire 终态不变。

`endEvent` 是本 Server 的成功终态证据。它可携带空 payload；若携带非空 payload，必须是可解析的 JSON，不能用 CRC 正确但语义畸形的终态结束一次响应。EventStream 解码器在 `feed` 发生结构或容量错误后，后续 `feed` 和 `finish` 都保持同一失败状态，不让结束调用把先前错误洗成成功。此门槛作用于本地流式和非流式转换，不推断厂商新的事件字段或改变 KI-N5 usage shadow、Compact/shared cache disabled 决策。

流式桥接器收到 `endEvent` 后仍继续读取上游响应体，直到真实 EOF 才发送 Anthropic `message_stop`。同块或后续块出现任何额外 EventStream 帧、截断帧或传输错误，均不能被提前的终态掩盖为成功；非流式路径保持相同的尾随帧拒绝合同。等待 EOF 仍受现有帧间 idle timeout 和请求生命周期内存预算约束。

EventStream 的 16 MiB 帧/待解码缓冲上限约束单个未完成帧，不把一次网络读取中的多个合法帧误当成一个超限帧。解码器边接收边排出完整帧；真正超限、CRC 错误或截断帧仍失败关闭。调用方的请求生命周期内存预算继续核算整块输入与解码后保留的帧，这不增加出站请求或放宽单帧上限。

同一 `toolUseId` 的 JSON 分片必须维持同一个非空工具名；起始分片缺名称时仍可由后续分片补齐，但两个不同的非空名称不能拼成一次调用。完成一次工具调用后，后续事件不得复用该 ID。两类冲突都以固定本地 `KIRO_EVENT_STREAM_INVALID` 失败，流式与非流式一致；正常同名分片、无参工具恢复和已停止但 JSON 无效时的既有处理不变。已完成 ID 集合纳入流生命周期内存预算。

工具调用在 `stop=true` 或 EOF 无参恢复时必须已经取得非空白名称；所有分片都缺名称不能成功输出空名称的 `tool_use`。首片有名而后续分片省略名称仍可沿用已绑定名称。无名完成在流式与非流式均以固定脱敏 `KIRO_EVENT_STREAM_INVALID` 拒绝，不把上游工具 ID 或名称写进客户端错误。

`assistantResponseEvent.content` 中字面 `<tool_use ...>…</tool_use>` 属于文本泄漏格式，不是结构化 `toolUseEvent`。下游文本过滤器跨分片移除完整或截断的真标签块；`<tool_user>` 等近似标签保持原样。真实结构化工具事件继续单独转为 `tool_use`，既有 `<function_calls><invoke>` 救援与去重仍保留。过滤只影响下游显示，不改变 Kiro 请求、EventStream 终态或计量权威。

## 只读来源

差异证据冻结在 `assets/contract/kiro-reference-delta.json`：append-only schema v2 保留 `kiro.rs@22d2c2d0695ba350890072c19990f54782827ae5` 的 committed prompt-cache 对象、`f2cc574` / `19b7f4b` / `47633a4` 历史对象，以及 `f413e7de` / `3194bb29` / `0b8c7dec` / `d62054f5` / `13763b69` 的 tool/profile/compact 增量；同时固定干净的 `kiro.rs@be0c04219d9d1b93b7fe5c3d7b9e7c9cf0d05863` committed-object snapshot、原 schema-v1 十字段摘要和 14 条不可变处置记录。第 14 条只把参考的 16 MiB EventStream frame/buffer ceiling 作为 retained-state 差分信号，`CORE-N2-KIRO=fixture_verified` 由本仓库独立实现支持。外部工作树不是构建、测试、发布或运行时依赖；默认审计只验证本仓库合同，显式 `--check-sources` 才按 commit/tree 与 SHA-256 读取这些 Git objects。
