# Share Token／USD 等值双口径额度

> 状态：权威实现文档；本地／夹具验证，不是生产验收证明。真实 Provider/App 转发面保持 `live_pending`。最后核对：2026-10-30。

## 1. 含义与启用条件

普通授权用户可选择 Token 数量或 USD API 标价等值额度。后者是使用资格，**不扣平台钱包，不代表 Provider 真实成本或应付账单**。Share 总 Token、用户到期、App 权限、并发与上游额度照常生效。市场托管授权禁止设置金额策略，既有试用／租金不变。

必须同时满足：

1. Router reader ≥10 且 heartbeat 显式声明 `usdEquivalentQuotaV1`，能力与 Router URL、installation 绑定，缓存有效期 300 秒。
2. 本地 SQLite authority 已经 `committed`；shadow 安装只能沿用 Token。
3. Router Owner 发布已核对的完整执行价目；Server 通过签名请求取得、校验、持久化，并收到匹配 ACK 后激活。默认无执行价目，不自动相信参考价目。
4. Share 只绑定已验证的原生 API-key profile：`claude.anthropic_api_key`、`codex.openai_api_key`、`gemini.google_api_key`，并核对实际 HTTP driver。当前 generic 文本转发面支持 Messages、Chat Completions、Responses、Gemini；Web Session、Coding Plan、其他自定义 profile、OAuth、媒体、内置付费工具、估算、未知模型／tier、多 attempt/retry 不在金额模式可用范围。仅凭兼容 ProviderType 为 Claude/Codex/Gemini 不足以放行。客户端自定义 function tool 文本允许；Server 不执行工具。

灰度部署还必须独立核对每个启用转发面 7 个自然日、至少 100 条真实完成文本请求。缺样本的面维持 Token；本地单测通过不等于 `live_verified`。

## 2. 策略、窗口和生效

wire：`policy.valueQuota={limitUsdMicros:"10000000",tariffPolicyId:"api-equivalent-v1"}` 表示 $10，和 `tokenLimit` 互斥，周期与 anchor 沿用 `tokenPeriod`。额度为正整数 micro-USD 字符串，最高 $1,000,000,000；不是 JS 浮点金额。

Server-owned `valueMetering` 保存 `quotaPolicyEpoch/valueMeteredFromMs/effectiveFromMs/pricingRevision`。首次没有完整自然窗口证据时，以 `pendingValueQuota={policy,effectiveFromMs,retainAccess}` 排期，原 policy 继续真正执行，不能先去掉 Token 限制再等待。新授权等待时不提供无限制访问。首次 lifetime 从确认的计量起点计量；已有未覆盖请求在途时拒绝首次即时生效。已有起点会持久保留，删除再新增同身份授权、改额度、切换单位不重置消费；epoch 不是清零桶。

自然日／周／月为 UTC；sevenDays/thirtyDays 是既有锚定固定窗口，不是滑动窗口。accountReset 需有效 source/epoch/bucket/cycle 和当前完整区间。按请求开始时间归属窗口，迟到修订不搬到完成时间的窗口。换周期必须核验新真实窗口覆盖，不能凭旧窗口的零推断新窗口无用量。

## 3. 准入与精确计量

`state/share_value_quota.rs` 统一编排；generic forwarder 在真实模型／tier 和最终 payload 已确定、发送上游前再次读真实授权并调用准入。模型选择、协议适配与策略更新之间的竞态不能绕过金额检查。

`repository/server_sqlite/value_quota.rs` 用 IMMEDIATE 事务读取当前窗口跨 epoch 的已用与风险，校验后持久化 attempt 才发网。额度达到 `used + pendingRisk >= limit` 时拒绝新请求；已准入请求允许结束，最终使用量可能超额，**这是请求级软上限，不是绝不超支的硬上限**。当前没有可信输出 ceiling，预占本窗口全部剩余容量；金额模式还取原用户并发与 4 的交集。普通 Token 并发规则不变。

费率单位是整数 micro-USD／百万 Token；内部 `1 USD=10^12 picos`，各分类直接 `tokens × rate` 累加，不逐请求截断成分或微美元。分类为 fresh input、output、cache read、cache write 5m／1h。模型元数据明确 raw/fresh 上下文阈值及比较方向；长上下文不能只看 fresh 输入。cache write 仅有合并量时使用已证明最高 TTL 费率并标记 conservative，没有可证明上界则 unavailable。缺价不等于零价，只有显式零价允许免费。

请求 admission 固定用户、Share、window、epoch、model、tier 和价目修订；`requestId/attemptId/usageRevision` 幂等。revision 0 保留给 admission，Usage 的 revision+1 连接观测；更高修订可合法调低累计，旧修订不能覆盖新值。同一 revision 不同 payload 拒绝。观测使用量和风险分离，部分流式更新不提前释放风险、不双计已观测消耗。HTTP 失败、499、504 仍计已观测量；未确认终结、估算、模型冲突、缺量保留风险，不能用状态码或进程重启释放成免费。

原生 Gemini 的模型在 URL 中：额度与日志使用该真实路径模型，不注入 body 模型，也不允许 body 字段覆盖它。原生 JSON／SSE 的上游模型证据与用量一起提交，不能先释放风险再异步记录模型不符；分片模型冲突保持到终态，证据仅保留一个不超过 250 字节的模型 ID 与冲突标志。冻结快照只信任实际执行 ID 或显式 exact alias；不要依赖同族／日期后缀的展示层匹配。上游报告了未声明的不同模型时，即使 token 数完整，也保留风险并拒绝该窗口后续 USD 准入；应先核对真实模型与价目，而不是清空风险。

Gemini 金额模式仅允许 `generateContent`／`streamGenerateContent`，`countTokens`、embedding、batch 等未覆盖动作在发网及金额预占前拒绝，不能将辅助计数误当成等待生成用量的 attempt。普通 Token 模式不改原行为。

后续用量帧没有模型字段时保留已记录的模型及其来源，包括旧 WebSocket→HTTP fallback 的证据；不能用空观测清除已有值或冲突标志。此兼容性修复不开放 WebSocket／OAuth 的金额模式。

Token 用户同步记录非阻塞 shadow 金额覆盖。缺价／未知面记录 gap，不拦 Token。shadow 持久化出错时写独立 `share-value-quota-degraded.json` 标记，只冻结 USD 的新启用和执行；不是用 Token quota 的全局故障标记连带停用普通请求。正常 Usage／金额结算在同一事务；如果金额检查点或修订损坏，通过 savepoint 回滚该次金额变更并保留旧风险，原始 Usage／Token 证据和数据库 USD-only 故障标记一起提交，再写外部标记。故障金额不得变为零，也不连带丢弃 Token 用量。若存储连安全标记也无法写入会记录 error，须先恢复存储并核对计量历史，不能自动宣称覆盖完整。

即使一次候选批量授权最终未提交，或免费 Share 的授权被删除，只要已有持久 coverage，后续未计价的请求仍记录 gap，防止删除再添加时被误认作完整零用量。免费 Share 中已撤销个人授权不再附带该个人的旧金额限制；私有 Share 则在发送前再次拒绝撤销竞态。

## 4. 价目和双端协议

线格式权威为 Router `PROTOCOL.md §13`。执行价目纯函数在 `domain/usage/value_pricing.rs`；Server 不运行时抓取 LiteLLM，也不读取 Router 钱包。快照 schema v1 最多 4 MiB／10,000 模型，准确模型和显式别名、声明 tier/context 的完整矩阵、规范十进制费率、重复 key 拒绝、规范 SHA-256。协议黄金向量在 `tests/fixtures/share-value-pricing-v1.json`，两仓各自携带并执行相同向量。

Server→Router 使用通用 installation Ed25519 envelope：

| POST 路径 | action |
| --- | --- |
| `/v1/shares/value-pricing/fetch` | `share_value_pricing_fetch` |
| `/v1/shares/value-pricing/ack` | `share_value_pricing_ack` |
| `/v1/shares/:share_id/value-quota-watermarks` | `share_value_quota_watermark` |

价目确认后在本地执行，Router 断连不使既有额度失效。调价只影响新的 attempt，旧 attempt 用冻结费率。旧 snapshot 和最低去重证据不得裁剪成不可追踪消费。

动态水位与静态 descriptor 分离；公平游标按 Share+email 轮转，每轮最多 16 次／约 5 秒、单请求 3 秒超时，单用户失败不阻塞其余用户；相同投影最多每 60 秒保活。Router ACK 后持久记录；ACK 超出本地真实水位则冻结该 scope，不能恢复旧库后把额度当零。金额 Share 无法协商时只隔离对应 Share 同步，不中断同批普通 Share。

`cacheCreationTtlV1` 独立协商。仅确认该能力时输出 `cacheCreation5mTokens/cacheCreation1hTokens`，两者同时出现、和等于 merged。历史未知 TTL 仍未知，reader v10／USD 能力不能代替 TTL 证明。

## 5. 状态、校准与隐私

Server Web 管理 invoke：`preview_share_value_quota`、`get_share_value_quota_status`、`edit_share_value_quota_usage`。Router 控制面对应 `/_ctl/preview_value_quota`、`/_ctl/edit_value_usage`，保持 HMAC／时间戳／nonce。预览只读，不创建 epoch、coverage 或消费记录；两端 UI 二次保存确认生效时间和范围。

状态为 observed、conservative、pending、unavailable、scheduled；金额缺失不显示伪零，90 秒未确认显示同步延迟。Server 管理界面展示 ledger 容量、待确认／缺证据请求和 gap 数；不为每个错误发送邮件。拒绝计数 `share_value_quota_rejections_total{reason=exhausted|price_unavailable|quota_unavailable}`。

金额校准独立于 Token `usageRebase/userUsageEdits`：set/clear 带 config CAS、grant revision、window、observed watermark、幂等 operationId、actor 和审计理由；set 不能低于真实已观测量，clear 只清人工偏移。批量同一事务全成或全败；活跃市场 grant 只读。用户 email 与合同额度不加入旧公开用量 API，Router owner／Share owner 和本人读取接口分别授权且 private,no-store。切账号清理旧内存状态。

## 6. 保留、恢复与运维

schema v3 新增独立 snapshots／coverage／attempts／revisions／adjustments／daily checkpoints，不改旧 Token/account quota 数字，也不修改市场钱款。每次 admission、gap、修订同步更新按 UTC 日的精确检查点，带校验和及记录数核对。完整日从检查点聚合，边界扫描最小明细；金额从不对 SQLite TEXT 做浮点 SUM。损坏／缺 checkpoint 失败关闭，不假装空窗口。

终态超过 30 天后每分钟最多压缩 100 条，保留 scope、最高修订、digest、已记金额和冻结 snapshot 引用，可处理合法迟到下降修订。pending／unavailable 不压缩。达到百万 attempt／数据库 4 GiB 告警，不静默删账或固定百万条截断。此容量告警不是已完成百万行压测的证明。

托管 backup restore 覆盖前先写独立 `share-value-quota-restore-fence.json`，标记当前 scope 的恢复边界，备份不得覆盖／删除独立 fence 或 degraded marker。旧当前窗口没有完整证据时阻止 USD，完整后续自然窗口才可再用；lifetime 或 Router ACK 领先须真实历史核对，不能靠清标记绕过。当前没有一键对账解除接口。离线手工替换数据库不在托管 fence 保证范围，下一次 Router ACK 可发现水位回退；禁止在线手工替换权威库。

回滚至不支持 v10 的二进制前，必须显式撤销或切回经过确认的 Token 策略，等待双端 ACK，保留在途和历史账证。不能将 unknown 字段剥离后把 USD 用户变成无限制。验证和生产部署是两个独立门槛；本次不部署远程、不截图、不执行 UI 自动化。

## 7. 本地验证记录（2026-10-30）

- `cargo clippy --locked --all-targets -- -D warnings` 通过；`RUST_MIN_STACK=67108864 cargo test --locked --quiet -- --test-threads=4`：3,936 项单元测试、132 项集成测试通过，默认 3 项 ignored，0 失败。10 万条容量夹具另行显式运行并通过；另外两项环境驱动测试未运行。
- 原生文本后端本地 HTTP 夹具覆盖四种入口 × 流式／非流式 × 正常／异常响应模型，共 16 组；验证发网前持久准入、精确 `$0.018` 结算、软超限后的拒绝、未知模型及未覆盖 Gemini 动作不发网且不制造金额占用。没有真实凭据或外部上游。
- Node 静态审计 1,911 项、纯 smoke 工具测试 13 项、Web 额度／批量策略／i18n 纯逻辑测试 29 项通过；Web 类型检查和生产构建通过。实际 `scripts/smoke/smoke-local.sh` 的本地健康、登录、Provider／Share 创建链路通过。
- 100,000 条最小终态 attempt 的 lifetime 累计精确、作用域索引命中；本机一次查询约 52 ms。这不是并发压测、吞吐或生产 SLO 证明。
- `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh`：`failures=0`、`decision=blocked`，因为聚合本地检查显式跳过且没有真实／部署输入；不能据此标记发布或生产验收通过。格式、生成契约、SQLite 权威边界、产品／Web 边界、文档索引及空白检查通过。

本地代码验证为 `local_verified`，真实流量验收保持 `live_pending`；没有执行截图、浏览器或其他 UI 自动化。
