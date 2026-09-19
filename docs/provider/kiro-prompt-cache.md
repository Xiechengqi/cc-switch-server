# Kiro Prompt Cache 本地计量合同

状态：`KI-01`、`KI-02`、`KI-03` 为 `fixture_verified`；CORE-N1 facade 与八路私有 receipt gate 已完成；`KI-04`、`KI-05` 仍为 `live_pending`，共享/远端缓存运行时关闭。

## 边界

该缓存只在 Kiro 没有返回权威 token usage 时模拟 Anthropic Prompt Caching 的下游计量字段。它不会改变发给 Kiro 的请求，不证明 Kiro 复用了推理前缀，也不降低或重算上游账单。每次调用仍固定在已解析的 Provider、Share、Account、签名用户与 credential generation；缓存 miss、持久化失败和重启都不能触发账号池、轮换、credential rail fallback、跨账号或跨 Provider fallback。

缓存 namespace 由调用链在进入本模块前固定，使用无歧义 JSON framing 覆盖 App、Provider id/revision、runtime fingerprint、Account id、`auth_identity_generation`、`token_refresh_generation`、Share、签名用户、route、runtime region 和 session。Claude 与 Codex 的 session precedence 继续由 `assets/contract/kiro-wire-protocol.json` 定义；缺 session 时使用请求级随机作用域，不能形成跨请求匿名命中。model、tool choice、thinking 和 output config 还会进入前缀哈希，防止不同推理合同复用同一个估算条目。

## KI-01：断点与 TTL

本地语义同时支持顶层 `cache_control` auto-caching 与 tool/system/message content 的显式断点。两类断点统一排序、去重并共享最多四个槽位。顶层 auto 在多轮请求中优先落到最新 user input 之前的最后一个可缓存 block，确保最新提问保持 uncached；只有首轮没有更早可缓存内容时才落到当前 user block。

每个断点向前最多检查 20 个 lookback position，只能命中此前真实写入过的断点。连续 `tool_use` block 合为一个 position，连续 `tool_result` block 也合为一个 position。thinking/redacted thinking 与空 text 不能作为断点。`ephemeral` 缺 TTL 或 `5m` 使用 300 秒，`1h` 使用 3600 秒；命中按条目自己保存的 TTL 滑动续期。混合 TTL 只允许 1h 断点位于 5m 断点之前。

非法 type/TTL、不可缓存断点、冲突的 message/block 声明、超过四个断点和反向混合 TTL 都整次禁用本地缓存估算，不截断声明或制造部分命中。`cc_switch_kiro_prompt_cache_decisions_total{decision}` 使用固定低基数原因记录 auto、explicit、no-cache 与 fail-closed 结果。

## KI-02：权威 usage 与比例守恒

本地缓存先在自身 estimator 口径计算三项：完整 prompt、最深断点覆盖前缀和最深已命中前缀。拿到 Kiro 的 `metricsEvent`/`contextUsageEvent` total 后，以纯整数、四舍五入方式先把覆盖比例映射到权威 total，再把命中比例映射到覆盖部分；剩余量确定性归入 uncached input。因此三项始终非负，并严格满足：

`input_tokens + cache_creation_input_tokens + cache_read_input_tokens == authoritative total`

只要上游 `messageMetadataEvent.tokenUsage` 存在，或 metrics 明确出现任一 cache usage 字段，上游值就优先，包括显式 0；缺失的另一个 cache bucket 视为 0，绝不以本地命中覆盖上游零值。响应 usage 用 `cache_usage_source=upstream_token_usage` 或 `local_prompt_cache_estimate` 公开来源；本地路径还给出固定低基数 `cache_usage_decision`。

## KI-03：SQLite 持久化

请求线程只更新有界内存表和 dirty mutation map，并用容量为 2 的 non-blocking 通知队列唤醒专用 writer。writer 在请求路径之外合并更新，失败时按 25ms 到 2s 有界退避；响应不会等待磁盘，也不会因写失败声称 durable。指标分别记录 coalesced、queue full、write success/failure、startup load 和 shutdown flush，`cc_switch_kiro_prompt_cache_persistence_degraded` 明确当前 durability 状态。

配置 `CC_SWITCH_KIRO_PROMPT_CACHE_PATH=/path/base` 时，新存储位于 `/path/base.sqlite`。旧 JSON 只在 SQLite 空库时读取一次，导入后保留原文件作为回滚输入，不再同步重写。SQLite 使用 WAL、`synchronous=FULL`、transactional upsert/delete、每条 mutation generation 与全局 generation CAS；过期清理和 4096 条容量限制在同一事务中完成。并发更新在内存中按 key/generation 合并，失败批次只在没有更新版本时回填。进程优雅退出在 HTTP drain 后执行有界 shutdown flush。

本地验收覆盖 SQLite transaction 冲突回滚、不可写路径降级、后台写不阻塞请求、restart TTL 命中、断点/TTL/lookback 语义和 usage 守恒。它不替代断电时的文件系统保证，也不把缓存可用性提升为请求成功条件。

## KI-04：真实验收

真实 receipt 必须按 auth kind 与 runtime region 分开，覆盖 `builder_id`、`idc`、`social`、`api_key` × `us-east-1`、`eu-central-1`。每份 receipt 固定当前 target commit、一个 Account auth/token generation、Claude/Codex 两个 Provider revision/runtime、一个 Share revision、签名用户/session、exact model 和两份 fresh catalog；执行 CountTokens 零推理发网、Claude/Codex non-stream/stream、tool namespace、auto/explicit/no-cache、5m/1h、四断点、20-block 边界、最新 user、上游 usage 优先、401/timeout/throttle/代际漂移、Compact 零发网、decoy 和 secret scan。OAuth kind 只允许同账号首次 401 pre-commit 重放，API Key 401 直接终止。

`scripts/smoke/kiro-real-receipt.mjs` 以 `--auth-kind` 与 `--region` 逐份验证仓库外 `0600` 私有 receipt；`scripts/audit/kiro-real-receipt.test.mjs` 已在 fixture 模式覆盖全部八组及漂移/伪造负例。fixture 只可得到 `contract_verified/live_pending`，不能生成真实通过。当前八份合同 receipt 仍全部为 `null`，全部保持 `live_pending`。

## KI-05：共享缓存门禁

共享/远端缓存当前 `runtimeEnabled=false`。只有明确的多副本需求、本地 SQLite 不足的运行证据、固定 namespace/version、TLS/secret 管理、短 timeout、熔断和本地降级设计全部完成后，才能另行评审。参考项目的 Redis、singleflight、session affinity、账号调度和命中驱动路由均未迁入；任何未来 remote hit 也不得改变固定 Account binding。

## CORE-N1：Provider lifecycle facade

`src/proxy/providers/kiro/` 现在承载 Kiro/Amazon Q 产品类型、canonical request/model/session、Account-bound catalog/region/request preparation、本地 CountTokens、keepalive、错误分类和 subscription throttle 决策。AWS EventStream/协议转换继续位于 `src/proxy/kiro.rs`；Share/Account lease、usage、terminal、统一 attempt budget 和同账号 401 replay 所有权继续位于共享 forwarder。该拆分不引入账号选择、跨产品共享 credential、额外 attempt 或 wire 变化。

## 只读来源

差异证据冻结在 `assets/contract/kiro-reference-delta.json`：`kiro.rs@22d2c2d0695ba350890072c19990f54782827ae5` 的 committed prompt-cache 对象与 `f2cc574`、`19b7f4b`、`47633a4` 历史对象继续保留；2026-09-18 另追加 `f413e7de`、`3194bb29`、`0b8c7dec`、`d62054f5`、`13763b69` 的 tool/profile/compact 增量。外部工作树不是构建、测试、发布或运行时依赖；默认审计只验证本仓库合同，显式 `--check-sources` 才按 commit 与 SHA-256 读取这些 Git objects。
