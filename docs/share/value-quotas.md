# Share Token／USD 等值双口径额度

> 状态：权威实现文档；本地／夹具验证，不是生产验收证明。真实 Provider/App 转发面保持 `live_pending`。最后核对：2026-11-02（Share Market USD 席位与美元试用预算，§12；本轮只做编译／静态验证，不运行测试）。

## 1. 含义与启用条件

普通授权用户可选择 Token 数量或 USD API 标价等值额度。后者是使用资格，**不扣平台钱包，不代表 Provider 真实成本或应付账单**。Share 总 Token、用户到期、App 权限、并发与上游额度照常生效。Router Share Market 托管席位也可以按 USD 等值额度出售，规则见 §12。

必须同时满足：

1. Router reader ≥10 且 heartbeat 显式声明 `usdEquivalentQuotaV1`，能力与 Router URL、installation 绑定，缓存有效期 300 秒。
2. 本地 SQLite authority 已经 `committed`；shadow 安装只能沿用 Token。
3. Router 从模型价格（LiteLLM 派生价 + 模型价格表中的管理员定价）自动派生并发布执行价目；Server 通过签名请求取得、校验、持久化，并收到匹配 ACK 后激活。
4. Share 所有绑定必须通过同一个 profile／driver 能力函数。允许 `claude.anthropic_api_key` 或 `claude.custom_http` + `http.anthropic_messages`；`codex.openai_api_key` 或 `codex.custom_http` + `http.openai_responses`；`codex.custom_http` + `http.openai_chat`；`gemini.google_api_key` 或 `gemini.custom_http` + `http.gemini_native`。generic 文本转发面支持 Messages、Chat Completions、Responses、Gemini。未列出的自定义协议组合、Web Session、Coding Plan、OAuth、媒体、内置付费工具、未知模型／tier 不在金额模式可用范围；发网后的重试同样不覆盖（唯一例外见 §3 的连接失败重试）。仅凭兼容 ProviderType 不足以放行；自定义 Claude 的实际兼容类型可为 `claude_auth`。客户端自定义 function tool 文本允许；Server 不执行工具。
5. 已启用且通过上述能力检查的 Single 绑定，其实际 upstream model 必须在当前 active 快照中具备 standard tier 价格。预览输出 `model_price_coverage`（模型／App／修订／排期时间），主动保存 USD 策略再次检查；只在排期快照有价时提示等待生效。Passthrough 输出 `scope=request_time`，不承诺所有调用方模型均有价，每次准入仍核实实际模型和 tier。后台保存已有授权不因该覆盖检查中断，也不清空历史。

灰度部署还必须独立核对每个启用转发面 7 个自然日、至少 100 条真实完成文本请求。缺样本的面维持 Token；本地单测通过不等于 `live_verified`。

## 2. 策略、窗口和生效

wire：`policy.valueQuota={limitUsdMicros:"10000000",tariffPolicyId:"api-equivalent-v1"}` 表示 $10，和 `tokenLimit` 互斥，周期与 anchor 沿用 `tokenPeriod`。额度为正整数 micro-USD 字符串，最高 $1,000,000,000；不是 JS 浮点金额。

Server-owned `valueMetering` 保存 `quotaPolicyEpoch/valueMeteredFromMs/effectiveFromMs/pricingRevision`。**只有存在生效中的 `policy.valueQuota` 或 `pendingValueQuota` 的授权才获得／保留 binding**；Token 授权不再做 shadow 金额计量，也不记录 coverage gap。旧版本给 Token 授权写入的 binding 原样保留但不再准入或同步，ledger 历史不删除。

生效规则（保存与预览共用同一函数）：

| 情况 | 生效 | 计量起点 | 预览 `reasonCode` |
| --- | --- | --- | --- |
| 已是 USD 且周期／anchor 不变（只改额度） | 立即 | 沿用原起点 | `ready` |
| lifetime／accountReset | 立即 | 当前时刻（已是生效中的 USD 时沿用原起点） | `metered_from_now`／`ready` |
| 自然日／周／月、sevenDays、thirtyDays，且 Server 能证明该用户当前窗口**零**已记录 Share 请求（新增授权天然满足） | 立即 | 当前窗口起点（精确，无遗漏） | `ready` |
| 自然日／周／月、sevenDays、thirtyDays，其他情况 | 下一窗口起点 | 保存时刻 | `scheduled_next_window` |

“零请求”证明使用与 Token 额度相同的每用户用量记录（`UsageStore::share_user_quota_usage`，日志＋汇总桶）：窗口起点至现在该 Share+email 的请求数与 Token 数都为 0。证明成立时 coverage 下调到窗口起点，使空 ledger 即为该窗口的精确历史。保存路径不等待 usage 锁（部分调用方已持有 usage 读锁，排队写者会让嵌套读死锁），拿不到快照时按“无法证明”处理，退回下一窗口排期；预览与保存共用同一函数。尚未写入用量日志的在途 Token 请求不在证明范围内（软上限容忍）。

排期期间原 policy 继续真正执行，`pendingValueQuota={policy,effectiveFromMs,retainAccess}`；新授权等待时不提供无限制访问。lifetime 首次切换明确“从现在起计量”，此前 Token 用量不折算。删除后重新添加同身份授权视为新授权（Owner 可用 `set` 校准）。同一 Share 批量保存时，属于用户本次编辑的授权准备失败会让保存失败；其他授权（例如后台提交时到期激活）失败只记 warn、保持原状，不中止整个 ShareStore 提交。

自然日／周／月为 UTC；sevenDays/thirtyDays 是既有锚定固定窗口。accountReset 需有效 source/epoch/bucket/cycle。按请求开始时间归属窗口。窗口 ID 由显式映射生成，与历史 `{:?}` 输出逐字节相同（如 `Day:<start>:<end>`、`Lifetime:0:0`）。

## 3. 准入、并发与结算

这是**请求级软上限**：宁可小幅、有界超额，也绝不误锁用户。

`state/share_value_quota.rs` 统一编排；generic forwarder 在真实模型／tier 和最终 payload 确定后、发网前调用准入。准入只在 shares 读锁下读取授权（不再持有全局 `share_quota_mutations`／`shares_persistence.gate`），SQLite 工作在 blocking 线程池执行；解析并校验过的价目快照按 (router, installation, revision) 进程级缓存（revision 即内容哈希，内容不可变），准入和结算共用。

`repository/server_sqlite/value_quota.rs` 在 IMMEDIATE 事务内：

1. 读取当前窗口跨 epoch 的已结算 `spent` 与在途 `pending`。`spent >= limit` → `VALUE_QUOTA_EXHAUSTED`；`spent < limit` 但 `spent + pending >= limit`（仅在途预占占满）→ `VALUE_QUOTA_BUSY`（可重试，避免把暂时状态当成到期锁死）。
2. 并发上限 = min(授权 `parallelLimit`，4)，按本 scope 窗口内 `pending` attempt 计数；超过返回 `VALUE_QUOTA_BUSY`，不是 EXHAUSTED。
3. 有界预占 `risk = min(available, price(inputEstimate 新输入 + outputCeiling 输出))`，按估算输入选择适用的上下文档位和请求 tier。`inputEstimate = ceil(请求体字节/3)`；`outputCeiling` 取 Anthropic `max_tokens`、OpenAI `max_tokens/max_completion_tokens/max_output_tokens`、Gemini `generationConfig.maxOutputTokens`，夹到 [1,128000]，缺省 32000。估算与预占写入 `meta` 的 `value_attempt:*`（不改 schema）。
4. attempt 身份为 (requestId, attemptId=attempt 序号)；冲突检查不含 policy epoch，策略编辑后的重试不冲突。

文本判定是协议感知的：只拒绝明确识别的独立计费内容——图片／音频／视频／文件／文档 part（`image`、`image_url`、`input_image`、`input_audio`、`input_file`、`document`、`file`，Gemini `inlineData/fileData`，非 TEXT 输出模态、`audio`/`speechConfig`）和服务端付费／内置工具（`web_search*`、`web_fetch*`、`file_search`、`code_interpreter`、`code_execution*`、`computer*`、`image_generation`、`mcp*`、`bash_*`、`text_editor_*`、Gemini `googleSearch*/codeExecution/urlContext/retrieval`、`cachedContent`）。cache_control、thinking／redacted_thinking、tool_choice、response_format／text.format、Responses reasoning／message／function_call(_output)／custom_tool_call(_output)、function／custom 工具、tool_use／tool_result 等均为文本。JSON schema、工具参数、签名等调用方数据子树不解释。

服务 tier 只在金额模式处理：上下文没有 tier 时记录 body 的**原始值**，不再用规范值覆盖导出日志；准入与结算共用 `canonical_service_tier`：缺省／`auto`／`default`／`standard` → standard，`priority`、`flex` 原样，其他（如 `scale`）无价。上游响应实际 tier 当前未被捕获（见 §10）。

### 结果分类结算

| 结果 | 金额 | 状态 |
| --- | --- | --- |
| 未发网（连接／DNS／TLS 失败、发网前的提前错误、客户端发网前离开） | $0 | observed |
| 上游返回错误 HTTP 状态（含 429/529）且无用量 | $0（有用量则正常计价） | observed |
| 完整、单 attempt、模型与 tier 证据一致 | 精确价 | observed（缓存 TTL 合并量为 conservative） |
| 多 attempt 但有最终用量；观测 tier 与准入不同 | 精确价（tier 取较高价） | conservative |
| 流中断（客户端取消、上游断开、停机）或截断 | 已观测部分用量，输出 = max(上报输出, ceil(已流出文本 UTF-8 字节/3))，以 outputCeiling 封顶；未观测输入用 inputEstimate | conservative |
| 结果未知（非流式发网后客户端断开、首字节超时、读体错误、模型／tier 证据不符、结算时缺价） | max(已观测用量的价格, 该 attempt 预占) | conservative |

已流出文本字节由流用量累加器统计（Anthropic text/thinking/partial_json、OpenAI content/reasoning/tool 参数、Responses `*.delta`、Gemini parts text），作为可选日志字段 `valueStreamedTextBytes` 保存（旧日志兼容，不导出给 Router）。

每个准入成功的 attempt 都会结算：forwarder 在准入后持有终结守卫，未被用量日志结算前若 `forward()` 提前返回或 future 被丢弃，就按阶段（未发网／上游错误／未知）异步调用 `settle_value_attempt_outcome`。守卫结算以 `synthetic:` 摘要写当前修订，任何真实用量日志（同修订或更高修订）都会取代它；同一 attempt 的第二次结果结算是 no-op，不会双计。窗口状态只有 observed／conservative／pending（以及 blocked）；`unavailable` 不再是终态，旧行由清扫器或对账解决。

免费 attempt 的重试：重试前 forwarder **同步等待**前一 attempt 的结算（`ValueAttemptGuard::settle_now`，结算后解除 drop 钩子），再 `continue 'attempt`，因此重试准入不会因旧行仍计入并发／预占而得到 `VALUE_QUOTA_BUSY`。前一 attempt 结算为 $0 时重试作为新 attempt（attemptId+1）重新准入：
- Claude 连接失败（`is_connect()`，请求未写出）；
- HTTP 200 流在任何业务输出之前出现上游错误：Anthropic `error` 事件、语义／协议错误（`semantic_protocol_error`）、首事件读取失败、Responses 语义失败帧（overflow／capacity 重试）、Claude body-stage 重试。这些 attempt 标记为上游错误（`mark_upstream_error`），无用量日志时按 $0 结算；若随后有真实用量日志，按其用量计价。
其它发网后重试（首字节超时、发网后的发送错误、按状态码的重试等）结果未知，仍拒绝金额模式，但同样先同步结算再重试。

清扫器（`state/background.rs`）每 5 分钟运行，启动时立即运行：对开始时间早于 max(进程启动时刻, now − (最长 Provider 请求超时 + 5 分钟)) 仍为 pending（含旧 `unavailable`）的 attempt，若有持久化用量日志则按上表分类（未结束的流视为中断），否则按“结果未知”收取预占，并以 info 记录数量。候选在**只读事务**中选出（WAL 读者不阻塞写；状态扫描无索引，schema 冻结不加索引），先 `pending` 后旧 `unavailable`，每次最多 400 条；随后以每批 25 条的短 IMMEDIATE 写事务结算。每条 attempt 在自己的 savepoint 中结算，失败（如检查点损坏）只回滚该条并阻断该 (scope, window)，其余照常提交。准入并发只统计**本 scope 当前窗口**的 `pending` 行；旧 Token shadow 遗留的 `unavailable` 行不计入并发、无预占，不会阻挡任何用户。

原生 Gemini 的模型在 URL 中：额度与日志使用该真实路径模型。Gemini 金额模式仅允许 `generateContent`／`streamGenerateContent`。

## 4. 价目和双端协议

线格式权威为 Router `PROTOCOL.md §13`。执行价目纯函数在 `domain/usage/value_pricing.rs`；Server 不运行时抓取 LiteLLM，也不读取 Router 钱包。快照 schema v1 最多 4 MiB／10,000 模型，规范 SHA-256。协议黄金向量在 `tests/fixtures/share-value-pricing-v1.json`。

Server→Router 使用通用 installation Ed25519 envelope，四个金额端点的 payload 一律以**规范 JSON** 签名并发送：递归按字节序排序所有对象键（`clients/router/client.rs::canonical_json_value`）。本仓启用了 serde_json `preserve_order`，否则嵌套 `status` 会按字段顺序签名而被 Router 的排序重序列化验签拒绝。双端共享向量 `tests/fixtures/share-value-signed-payload-v1.json`。

| POST 路径 | action |
| --- | --- |
| `/v1/shares/value-pricing/fetch` | `share_value_pricing_fetch` |
| `/v1/shares/value-pricing/ack` | `share_value_pricing_ack` |
| `/v1/shares/value-pricing/active` | `share_value_pricing_active`；需 Router 声明 `valuePricingActivationV1` |
| `/v1/shares/:share_id/value-quota-watermarks` | `share_value_quota_watermark` |

价目拉取 payload 为 `{"knownRevision":"<本地最新已安装 revision>"}`（已安装并 ACK 的排期 revision 优先，否则 active；尚无安装时为 `{}`）。Router 的发布 revision 是**全局**的（不区分 installation）：`knownRevision` 等于 Router 当前发布 revision **且本 installation 有 ACK** 时回 `{"unchanged":true,"revision":"<rev>"}`，否则回完整响应。unchanged 时不安装、不 ACK、不改 Share；完整响应为已安装的修订时仍重 ACK，以恢复 Router 丢失的证据。Router 重新发布旧 active 修订时，取消与之不符的 pending，避免激活已撤回调价。新 revision 安装、ACK、激活后也不需要改写 binding。调价只影响新的 attempt，旧 attempt 用冻结费率。

- **旧 Router 兼容**：本变更之前构建的 Router 对任何非空 fetch payload 回 400（“fetch payload must be empty”）。Server 收到携带 `knownRevision` 的请求被 400 拒绝时立即用 `{}` 重试，并在内存中按 Router URL 记住“不支持 knownRevision”；进程重启或 Router heartbeat 能力声明 `valuePricingKnownRevisionV1` 时清除。
- **未来生效的价目**：Router 最新 revision 尚未到 `effectiveFrom` 时，Server 收到即校验、存储并 ACK，在 `meta` 记录排期（`value_tariff_pending:*`，不改 schema）。独立本地任务每秒激活到期排期，准入 IMMEDIATE 事务再次检查并原子选择修订，**不再等待 heartbeat**；断连／重启仍可激活。期间旧 active revision 继续用于准入，`knownRevision` 发送已排期 revision，Router 回 unchanged，不再每次重下最多 4 MiB 的快照。已有 attempt 重放沿用冻结修订；更新且已生效的 revision 会直接激活并清除排期。
- **激活证据独立于 ACK**：协商 `valuePricingActivationV1` 后，heartbeat 后用签名 `share_value_pricing_active` 上报 `{revision,storageSchema:3}`。失败只等待下一轮重报，不阻断水位。Router 对本 installation ACK、生效时间、签名时间和 180 秒新鲜度独立校验；只收到 ACK／旧 Server 未发 active 回执时不显示“已激活”。Router-first 部署；旧 Router 不声明能力时不调用 active 端点。本地元数据激活不依赖此回执成功。
- **ACK 丢失恢复**：Router 若在 descriptor 同步时发现金额授权绑定的价目 revision 未被本 installation ACK（如 Router DB 恢复／installation 清理），返回 409 `value_pricing_ack_missing`、`details.revisions`。Server 对其中本地已安装的 revision 重发与正常流程相同的签名 ACK，然后**只重试一次** descriptor 同步；本地未安装的 revision 记 warn、授权保持原样。再次失败交回调用方正常的重试节奏（不会热循环）。

动态水位与静态 descriptor 分离；只同步金额授权（生效或排期中），公平游标按 Share+email 轮转，每轮最多 16 次／约 5 秒、单请求 3 秒超时；相同投影的保活阈值为 45 秒（低于 60±6 秒的 heartbeat 间隔，空闲用户每次 heartbeat 都会同步；Router 侧把投影的陈旧阈值提高到 180 秒）。

`cacheCreationTtlV1` 独立协商，规则不变。

## 5. 状态、错误码、校准与隐私

Server Web 管理 invoke：`preview_share_value_quota`、`get_share_value_quota_status`、`edit_share_value_quota_usage`；Router 控制面 `/_ctl/preview_value_quota`、`/_ctl/edit_value_usage`。预览只读，不创建 epoch、coverage 或消费记录。预览顶层与每行都带英文 `reason` 和稳定机器码 `reasonCode`：`ready`、`scheduled_next_window`、`metered_from_now`、`unverified_surface`、`pricing_unavailable`、`storage_unavailable`、`router_upgrade_required`、`blocked_needs_reconcile`、`invalid_policy`。预览中的策略校验错误按行返回 `invalid_policy`；管理端点上剩余错误映射为 400（校验）／409（revision 冲突）／404／503（存储），不再是 500。

状态行 `meteringStatus` 为 observed、conservative、pending、blocked、scheduled；单个用户读取失败时只该行返回 `unavailable` 与 `reasonCode`，不让整张列表失败。当前 policy 已是生效中的 USD 时，一律报告当前窗口的真实状态（即使另有排期中的 USD policy，校准仍可用）；只有当前 policy 不是 USD（或尚未生效）且有排期 USD policy 时才报 `scheduled`。排期信息放在独立字段 `pendingValueQuota={effectiveFromMs,limitUsdMicros}`（仅 Server UI 行，不进入 Router 投影）。

用量校准（`edit_share_value_quota_usage`、`/_ctl/edit_value_usage`）的错误：非生效 USD 授权或窗口不可用 → 409 `value_quota_not_active`；目标非法（低于已观测、越界、clear 带 target、批量重复等）→ 400 `quota_target_invalid`；revision／operation 冲突 → 409 `quota_revision_conflict`；Share 不存在 404；存储 503 `quota_unavailable`；不再有 500。编辑响应中读取失败的行返回 `{"email","error":{"code"}}` 而不是降级状态行，客户端合并时不会覆盖正确数据。

推理错误码：

| 条件 | HTTP | code | 可重试 |
| --- | --- | --- | --- |
| 已结算用量达到额度 | 403 | `value_quota_exhausted`，消息含 `resets at <RFC3339>`（lifetime 无重置），错误 details 带 `resetsAt` | 否 |
| 并发上限或在途预占占满 | 429 | `value_quota_busy`，`Retry-After: 2` | 是 |
| 私有 Share 授权已撤销 | 403 | `cc_switch_forbidden` | 否 |
| 无已确认价目 | 503 | `price_unavailable` | 否 |
| SQLite busy／locked | 503 | `quota_unavailable`，`Retry-After: 5` | 是 |
| 其他（窗口待对账、未就绪等） | 503 | `quota_unavailable` | 否 |

指标 `share_value_quota_rejections_total{reason=exhausted|busy|price_unavailable|quota_unavailable}`。准入错误为类型化 `ValueQuotaRejection`，映射方 downcast，不再匹配字符串。

金额校准独立于 Token `usageRebase/userUsageEdits`：set/clear 带 config CAS、grant revision、window、幂等 operationId、actor 和审计理由。**不再要求精确 `expectedWatermark`**（任何请求都会推进水位，Owner 永远无法保存）；改为在应用时校验 target ≥ 已观测量，字段保留并写入审计。批量同一事务全成或全败；活跃市场 grant（含 USD 席位）只读，不提供人工校准。

**对账 = 对某窗口执行 `set`**，同一事务内：(1) 把该窗口中开始时间早于清扫器同一陈旧阈值（max(进程启动, now − (最长请求超时 + 5 分钟))）仍 pending（含旧 unavailable）的 attempt 按“结果未知”结算，更年轻的 pending 行可能仍在进行，保持原样（其预占照常计入）；(2) 从 attempt／gap 行重建该 scope 的全部日检查点；(3) 若阻断记录带有 Router 水位（Router ACK 领先），先把全局水位计数器 `value_quota_watermark_v1` 抬到至少该值，使本次调整记录的水位为 Router 水位 + 1 以上，下一次同步领先而不会再次阻断；(4) 清除该 scope/window 的阻断、旧的 scope 级阻断和旧全局标记；审计记录写明处理数量与 Router 水位。提交后再清除该 scope 的恢复 fence（文件，幂等）。

## 6. 保留、恢复与运维

schema v3 表结构不变（大规模 ledger 简化／schema v4 延后）。每次 admission、修订同步更新按 UTC 日的检查点，带校验和及记录数核对；损坏的检查点只让该用户准入失败，经 `set` 对账重建。金额从不对 SQLite TEXT 做浮点 SUM。

故障作用域：
- 结算无法应用时，只回滚 USD savepoint（Usage／Token 证据照常提交），并写 `meta` 键 `value_quota_blocked:{scope}:{windowId}`；其他用户、其他窗口不受影响。旧的全局 `value_quota_degraded_v1` 不再写入也不再强制。旧版 shadow 计量写入的 `share-value-quota-degraded.json` 不再加载或强制：启动时删除并记 info（预览 `metering_storage` 只看 Token 体系的 `quota_usage_degraded`）。连作用域阻断都写不进时，整条用量写入失败并按普通存储错误处理。
- Router ACK 水位领先本地时只阻断该 scope 的该窗口（后续窗口照常），阻断记录（JSON）保存 Router 已确认的最高水位，供 `set` 对账越过它。
- 托管 backup restore 前写入的 `share-value-quota-restore-fence.json` 仍保留：fence 覆盖的窗口状态报告 `blocked`、准入拒绝；Owner 核对真实历史后对该窗口执行 `set` 即清除该 scope 的 fence。

终态超过 30 天后每分钟最多压缩 100 条（同时删除对应 `value_attempt:*` 估算），pending 不压缩。`diagnostics` 为管理端读，按 shareId 做 json_extract 全扫描，不在请求路径上。

回滚至不支持 v10 的二进制前，必须显式撤销或切回 Token 策略，等待双端 ACK。验证和生产部署是两个独立门槛。

## 7. 本地验证记录（2026-10-30）

- `cargo clippy --locked --all-targets -- -D warnings` 通过；`RUST_MIN_STACK=67108864 cargo test --locked --quiet -- --test-threads=4`：3,936 项单元测试、132 项集成测试通过，默认 3 项 ignored，0 失败。10 万条容量夹具另行显式运行并通过；另外两项环境驱动测试未运行。
- 原生文本后端本地 HTTP 夹具覆盖四种入口 × 流式／非流式 × 正常／异常响应模型，共 16 组；验证发网前持久准入、精确 `$0.018` 结算、软超限后的拒绝、未知模型及未覆盖 Gemini 动作不发网且不制造金额占用。没有真实凭据或外部上游。
- Node 静态审计 1,911 项、纯 smoke 工具测试 13 项、Web 额度／批量策略／i18n 纯逻辑测试 29 项通过；Web 类型检查和生产构建通过。实际 `scripts/smoke/smoke-local.sh` 的本地健康、登录、Provider／Share 创建链路通过。
- 100,000 条最小终态 attempt 的 lifetime 累计精确、作用域索引命中；本机一次查询约 52 ms。这不是并发压测、吞吐或生产 SLO 证明。
- `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh`：`failures=0`、`decision=blocked`，因为聚合本地检查显式跳过且没有真实／部署输入；不能据此标记发布或生产验收通过。格式、生成契约、SQLite 权威边界、产品／Web 边界、文档索引及空白检查通过。

本地代码验证为 `local_verified`，真实流量验收保持 `live_pending`；没有执行截图、浏览器或其他 UI 自动化。

## 8. 就绪诊断和受控启用（2026-10-31）

预览返回 `capabilityReady` 与 `checks[]`，独立报告 `router_protocol`、`sqlite_authority`（committed／prepared／shadow_verified／unavailable）、`metering_storage`、`acknowledged_tariff` 和每个**已启用 app** 绑定的 `text_surface`（app／profileId／driverId）；Provider Bundle 中已绑定但未启用的 app（常见于该 app 没有凭据）不承载请求，不参与就绪和保存校验，转发时每个请求仍单独校验运行面。任一基础条件失败均返回全部诊断及空 rows，不继续查询用户窗口；config revision 不匹配仍返回冲突。基础能力与用户 coverage 分开，不能因一个用户历史不完整禁用全体用户的单位选择。Server Web 和 Router 都可重新检查，后台检查不覆盖显式策略确认预览。

执行价目不再单独编辑：Router 在启动和每次改价后，把「设置 → 模型价格」（LiteLLM 派生价 + 管理员为未定价模型填的价）冻结成快照并自动发布，替换已有价目时 60 秒后生效。快照按价格显示的回退链物化各 tier／上下文行，长上下文按含缓存的输入判定，缺 1h 写价按 5m 计；美元额度扣的就是用户看到的价格。Server `PricingSnapshot::model` 与显示使用同一模型身份剥离（厂商前缀、路径、`-thinking`／`[1m]`／`-v1:0`、日期戳等），共用 `tests/fixtures/share-value-model-identity-v1.json`。未定价模型美元额度用户无法调用，需在模型价格表定价。最新发布与当前 active 快照分开，等待未来发布不使旧快照失效。

现有 `shadow_verified` 不代表完成 SQLite 权威迁移。空数据目录首次启动直接提交；已有数据的 Server 由管理员在「设置 → 通用 → 数据存储」点「迁移并重启」（一次性请求，下次启动在目录锁内执行，`prepared` 前失败则保留 legacy 并显示原因），或按 [官方 Server SQLite Authority 迁移](../guide/data-migration.md#server-sqlite-authority) 停服运行 `config migrate-server-store --apply`。禁止在线强改 marker。迁移后重启并验证旧授权、Provider 和用量。

未提交时，价目同步与水位同步在 `router_supports_value_quotas` 直接跳过（每进程记一条 info 日志）；`get_share_value_quota_status` 省略 `diagnostics` 而不是返回 500。预览中 `acknowledged_tariff` 失败时带 `blockedBy`：存储未提交为 `sqlite_authority`，Router 未协商为 `router_protocol`，经 Router 代理且价目未发布时为 `tariff_published`（Router 补充）。界面只把无 `blockedBy`（或其阻塞项已通过）的失败项当作根因，按 router_protocol、sqlite_authority、metering_storage、tariff_published、acknowledged_tariff、text_surface 排序，行内显示首个根因及 +N，其余列为「等待：…」。

部署顺序为 Router 再 Server。Router 自动发布价目，等 Server 安装及 ACK 激活后，在对应 Share 重新检查；用户策略仍须单独确认覆盖起点或排期。故障不能靠删除 ledger、恢复旧库或清除安全标记绕过。此代码变更未自动切换任何生产存储或发布估值；真实灰度继续 `live_pending`。

## 9. 本轮验证与整体 review（2026-10-31）

- `cargo clippy --locked --all-targets -- -D warnings`、格式及空白检查通过。`value_quota_` 定向测试 18 项通过、1 项既有容量测试默认 ignored；价目域纯测试 8 项也通过（与定向测试部分重叠，不重复累计）。本轮未重跑第 7 节的全量 Rust 测试或 10 万条容量测试。
- 本地 HTTP 夹具扩展为 8 个原生／自定义入口组合 × 流式／非流式 × 正常／异常模型，共 32 组。正常精确结算 `$0.018`，异常模型保留风险；发网前持久准入、缺价和未覆盖 Gemini 动作拒绝仍通过。无真实凭据或外部 Provider。
- Web 额度／批量策略／传输／i18n 纯测试 34 项、类型检查及 Vite 生产构建通过，`web-dist` 与源同步。既有大 chunk 提示不作失败，构建体积审计通过。Provider 覆盖、UI 矩阵、文档索引、SQLite 权威、产品／Web 边界、i18n literals 和运行时契约审计通过。
- Review 发现并修复自定义 HTTP 响应模型证据遗漏：这些 profile 的报告模型和 sticky conflict 现在进入持久计量路径，但不会因此绕过 profile／driver 准入。两端就绪检查与显式策略确认独立，旧账号／Share／revision 响应不能恢复旧能力；非法预览或配置冲突不能当成功处理。

这是本地代码验证，不是生产或视觉验收；没有截图、浏览器／UI 自动化、远程迁移或部署，真实灰度保持 `live_pending`。截至本轮收尾未发现已知阻塞性代码问题，修改尚未提交或推送。

## 10. 第二轮修正（2026-10-31）

本节对应上文 §2–§6 的新语义：有界预占与每用户并发、结果分类结算与终结守卫、清扫器、作用域阻断与 `set` 对账、取消 shadow 计量与新的生效规则、类型化错误码、规范签名 payload 与 `knownRevision` 拉取。schema 与迁移校验和未改动。

已知限制：上游响应中实际服务的 tier（例如 OpenAI `auto` 被提升为 priority）目前未被任何管道捕获，结算只能使用请求 tier；若需按响应 tier 计价，需要单独增加响应 tier 证据字段。SQLite 连接仍按调用打开（未做连接池，以免备份恢复替换数据库文件后复用旧句柄）。本轮只做 `cargo check`／格式检查，未运行测试。

## 11. 第三轮修正（2026-10-31）

对应上文 §2–§6：旧 Router 的 `knownRevision` 400 回退、descriptor 409 `value_pricing_ack_missing` 的 ACK 恢复（单次重试）、Router 水位领先后的 `set` 越过水位、移除旧 degraded 标记、重试前同步结算与预发上游错误免费重试、清扫器逐行 savepoint／读写事务分离／pending 优先、`meteringStatus` 只在非 USD 生效时报 scheduled、校准错误码与编辑响应错误行、保活 45 秒、未来价目先安装并 ACK、用量日志只结算匹配 attempt、`set` 使用清扫器陈旧阈值、证明为空的自然周期立即生效。schema 与迁移校验和未改动。本轮只运行 `cargo check`／`cargo clippy`／格式检查，新增和修改的测试未运行。

## 12. Share Market USD 席位（托管授权）

Router 托管授权（`manager=routerShareMarket`）的 managed upsert 可携带 `policy.valueQuota`，与 `tokenLimit` 互斥，在任意市场阶段（`trial`、`paid` 或无阶段的免费席位）都可接受；`accountReset` 仍被拒绝。新试用预算跟随席位单位：USD 字段 `trialValueLimitUsdMicros`／`trialValueAllowanceUsdMicros` 与 Token 字段互斥；旧冻结 Token 试用仍按原规则。试用预算与席位自身的周期额度同时执行，详见 [试用协议](market-trials.md)。

计量按合同（`entitlementId`）隔离，Market ledger scope 为 (Router, installation, Share, email, entitlementId)；普通授权不序列化 entitlementId，旧账本键保持不变：

- 合同第一次以 USD 策略应用时新建 binding：`valueMeteredFromMs = effectiveFromMs = 应用时刻`，`begin_coverage(now)`。同一 email 此前的合同或普通授权在同一窗口的用量因 entitlement scope 被隔离，同毫秒和迟到结算也不会串账。托管授权**从不**排期（`pendingValueQuota`），不使用“零请求”窗口证明，也不把 coverage 回拨到窗口起点。
- 同一 entitlement 之后的 upsert（trial → paid、计费暂停 revoke 后恢复、App 范围升级、完整性修复重发）沿用原 binding（计量起点与 epoch 不变），只刷新 `pricingRevision`；只有金额、`tokenPeriod` 或 anchor 变化时才生成新的 `quotaPolicyEpoch`。sevenDays／thirtyDays 在付费 upsert 时重新物化 anchor，窗口随之从付费开始重算；自然周期中试用期用量计入同一窗口，与 Token 模式一致。不同 entitlement（即使同一 email）一律新建 binding。
- 新建或策略变化需要与普通 USD 策略相同的门槛（Router v10＋`usdEquivalentQuotaV1`、SQLite authority 已提交、已启用绑定全部通过文本计量运行面、已激活价目）。不满足时 managed 操作失败（`VALUE_QUOTA_UNSUPPORTED_PROVIDER_SURFACE`／`VALUE_QUOTA_ROUTER_UPGRADE_OR_COMMITTED_STORAGE_REQUIRED`／`VALUE_QUOTA_SNAPSHOT_UNAVAILABLE`），Router 拉取路径与 `/_ctl/apply_share_settings` 都经由同一 ShareStore 提交（`prepare_value_bindings_with_context`），因此活动的托管 USD 授权不会无 binding 落盘。已绑定授权的后台重新准备失败只记 warn，不中止整个 ShareStore。

准入（`admit_share_value_text`）、状态行与水位同步对托管授权与普通授权一视同仁；有效并发仍为 min(`parallelLimit`, 4)。用量校准（`edit_share_value_quota_usage`、`/_ctl/edit_value_usage`）对托管授权返回 400 `quota_target_invalid`；普通预览中的托管用户行仍为 `invalid_policy`（只读）。

能力声明：预览 `checks[]` 末尾固定包含 `{"code":"market_seat_quota","ready":true}` 和 `{"code":"market_trial_value","ready":true}`，分别表示本构建支持 USD 席位和一次性 USD 试用。它们始终 ready，不影响 `capabilityReady`／`reasonCode`；Router 缺失相应声明时禁止 USD 席位／新的正金额 USD 试用。Server Web 只列出失败的检查，因此这两项不显示；托管授权行与普通 USD 授权一样显示美元额度与周期，但不可校准。

一次性试用在周期 admission 的同一事务内附加合同／scope 预算检查，在途预占一并扣除；试用索引保留跨合同金额，且每次终态结算原子保存最高试用金额，保证“先保守结算、后真实下修”及崩溃／压缩也不重新开放免费预算。正常周期 meter 仍按最新真实金额；动态 `trialValueUsedPicos` 使用最高值镜像，与 Token 共用 `usageRevision`。缺持久化 Market request receipt 或更换 entitlement 的竞态拒绝发网。

Router 的“本期 API 等值／实际折扣”是 Router 保留日志与窗口末端目录价格的展示估算，不是该账本的权威额度，不改变任何钱包和 Provider 结算。标称折扣按 30 天满额折算，Token／lifetime／免费席位不推算。

本节只做 `cargo check`／格式检查，新增测试未运行。
