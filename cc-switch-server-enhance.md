# cc-switch-server 反代增强分析与实施计划

> 状态：一次性差异分析与实施规划，不是架构或协议真值。架构仍以 `docs/architecture/overview.md` 为准，Provider 身份与能力仍以 `assets/contract/provider-registry.json` 为准，wire 证据仍以 `PROTOCOL_EVIDENCE.md`、厂商协议和本仓库冻结 fixture 为准。
>
> 分析日期：2026-09-11。目标基线：`cc-switch-server@993fca19a6c2`，分析时工作树干净。

## 0. 实施结果与最终 review（2026-09-11）

本节记录本文计划在当前工作树的实施结果；后续章节保留最初的差异分析、设计理由和证据索引。状态含义如下：

- `fixture_verified`：生产路径已实现，并由本仓库自包含 fixture、合同审计或故障测试验证；不代表真实订阅 entitlement。
- `live_pending`：离线实现或验收合同已就绪，但没有本仓库真实凭据 receipt，不能提升为 `live_verified`。
- `runtimeEnabled=false`：能力只有 fail-closed 门禁或风险占位，运行时不开放；这是计划要求的安全结果，不是用 fixture 冒充支持。

### 0.1 八类 Provider 落地状态

| Provider | 已完成并离线验证 | 证据门禁结果 |
| --- | --- | --- |
| Antigravity | `AG-01` reasoning replay、`AG-02` conversation-scoped session/受控 rollover、`AG-03` schema/transport/quota/Retry-After 均为 `fixture_verified` | `AG-04` compaction 为 `live_pending` 且 `runtimeEnabled=false`；`antigravity_oauth`、`agy_oauth` 分 rail 保持 `live_pending` |
| Claude | `CL-01` leading system、`CL-02` 请求类别 cache TTL、`CL-03` trailing usage、`CL-04` structured output/interleaved tools/helper request id 均由本地合同覆盖 | 真实 Anthropic 接受性没有 receipt，相关 operations 保持 `live_pending` |
| Codex | `CX-01` Unicode schema、`CX-02` const union、`CX-04` HTTP/SSE/WS 错误与 fallback golden 为 `fixture_verified` | `CX-03` GPT Image 2.5、`CX-05` WS prewarm、`CX-06` image quota/cache 均为 `live_pending` 且不开放运行时能力 |
| Cursor | `CUR-01` 合同真值、`CUR-03` protobuf/SDK 周期差分为 `fixture_verified`；forward/test/discovery 合同已统一 | `CUR-02` OAuth/API-key 双 rail 仍分别需要真实 receipt，保持 `live_pending` |
| Grok | `GR-01` reasoning replay、`GR-02` 明确错误驱动恢复、`GR-03` root union adapter 为 `fixture_verified` | `GR-04` 真实推理/媒体矩阵为 `live_pending`；`GR-05` remote compaction 为 `live_pending`、`runtimeEnabled=false` |
| Kiro | `KI-01` cache 语义、`KI-02` usage 守恒、`KI-03` 异步 SQLite cache 为 `fixture_verified` | `KI-04` 各 auth kind/region 为 `live_pending`；`KI-05` 多副本共享 cache 为 `live_pending`、`runtimeEnabled=false` |
| Qoder | `QD-01` contract revision/状态、`QD-03` CLI oracle 升级审计为 `fixture_verified` | `QD-02` Global OAuth、Global PAT、CN OAuth 三 rail 分别保持 `live_pending` |
| CodeBuddy | `CB-01` 空 message、`CB-02` 空 delta、`CB-03` tools/tool_choice 归一为 `fixture_verified` | `CB-04` Intl/CN 双站保持 `live_pending`，企业/多模态未被外推开放 |

每类 Provider 都有独立的 `assets/contract/*-reference-delta.json` 与对应 audit。外部参考仓库的 commit/object/hash 只用于可选的只读来源复核；本仓库构建、测试、CI 和运行时均不读取这些仓库。

### 0.2 横切增强结果

| 项目 | 结果 |
| --- | --- |
| 固定绑定与恢复边界 | 已完成。`AttemptBudget`、`CommitGuard`、`BindingSnapshot`、分阶段上限、总次数/耗时预算和低基数 decision 指标已接入；恢复仅允许同 Provider、同 Account、同 rail、身份代际不漂移且下游业务输出提交前。生产 `next_provider_failover`、`ProviderFailover`、`excluded_provider_ids`、`after_provider_failover`、`select_failover_provider` 入口已删除，并有静态回归门禁。 |
| Replay/cache 基础件 | 已完成。Antigravity/Grok 使用 Provider 专属 scope/value 规则与 snapshot CAS；命中所有权、TTL/容量、并发冲突、过期、分支和身份代际漂移均有测试，敏感 opaque 内容不进入日志或 receipt。 |
| execution 核心拆分 | 核心状态机边界已完成：`src/proxy/execution/{context,recovery,terminal,transport}.rs` 分别承载固定绑定、统一预算、提交栅栏和 WS fallback 的取消/Ping-Pong 原语，Provider 方言仍在专属模块。`ForwardAttemptContext` 仍在 `forwarder.rs` 组合这些原语，因此不能宣称该文件已经缩成纯 dispatch；继续物理拆分是可独立进行的结构优化，不影响本次已验证的恢复不变量。 |
| Kiro 热路径存储 | 已完成。同步整份 JSON snapshot 已由 bounded async writer、dirty generation、合并/退避、shutdown flush 和 SQLite generation CAS 取代；磁盘不可用不阻塞请求或改变内存语义。 |
| `STORE-01`～`STORE-05` | 已完成。Provider/Account/Share/Usage 有 SQLite schema、引用图事务、generation/revision CAS、usage UPSERT、shadow 校验和 `shadow_verified → prepared → committed` authority 状态机；覆盖 `SQLITE_FULL`、stale snapshot、并发 backup、语义篡改、WAL 截断/垃圾/bit flip。 |
| 迁移、备份与回滚 | 已完成。新增 `config migrate-server-store` 预检、`--apply` 幂等切换和 `--rollback-export DIRECTORY` 离线导出；committed backup 会校验逐表数量、generation hash、订阅引用图与完整 WAL checksum。默认首次启动只建立/验证 shadow，显式 CLI 或 `CC_SWITCH_SERVER_SQLITE_AUTHORITY=committed` 才切换；prepared/committed 会自动 roll-forward，失败 fail closed。 |
| Conformance/receipt | 已完成合同闭环。单向验证等级、`live_pending` 附加状态、receipt schema、敏感字段禁令、registry/coverage/UI 一致性均由生成或 audit 门禁约束；因为没有真实凭据，本轮没有产生或伪造 `live_verified`。 |
| 外部依赖隔离 | 已完成。reference delta 可核对冻结 Git object，但外部项目没有成为 Cargo/npm 依赖、测试输入、CI checkout 或运行时同步源。 |

### 0.3 最终验证记录

本轮最终 review 未发现阻断性问题。已完成的本地验证如下：

```text
cargo fmt --check
cargo check
RUST_MIN_STACK=16777216 cargo test
scripts/static-checks.sh
cargo build
KEEP_CONFIG_DIR=1 PORT=18083 scripts/smoke/smoke-local.sh
```

- Rust：library `3044 passed, 1 ignored`；API contract `124 passed`；Cursor fixture、lease contract 和 doc tests 通过。
- 静态门禁：Clippy `-D warnings`、71 项 Node audit tests、12 项 smoke helper tests、八类 reference delta、SQLite/Provider/registry/coverage/UI/docs/source/dependency/state-write audits 全部通过。
- Web：typecheck 通过；41 个 Vitest 文件、213 项 jsdom 单元测试通过。按任务约束未运行浏览器/UI 自动化。
- 本地 smoke：health、version、Web fallback、offline setup、密码/API token 登录、Provider 和 Share 创建通过。
- `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` 按设计返回 `blocked_inputs`：显式跳过内置 test 阶段、缺真实 Token/Share/Router/Provider 输入且没有部署测试。全量本地测试已由上面的独立命令完成；没有为了变绿运行真实账号或部署测试。

因此，本次完成的是八类反代的生产实现、离线合同、存储与安全门禁；所有必须依赖真实 entitlement 或部署环境的能力继续诚实保持 `live_pending` / `blocked_inputs`。

## 1. 结论摘要

`cc-switch-server` 已不是功能空壳：当前约有 36.3 万行 Rust，八类 Provider 均已有不同程度的账号、协议转换、流式终态、同账号恢复、用量和离线 fixture。相对成熟参考项目，最值得优先补强的不是增加更多 Provider，而是以下五类薄弱点：

1. **协议正确性仍有少数可复现缺口**：Codex 工具 schema 的 Unicode property escape、Grok Build 顶层 union、Claude native passthrough 的首条 system role、CodeBuddy 空消息/空 delta。
2. **多轮推理连续性不完整**：Antigravity 和 Grok 缺少各自协议专用、严格隔离且可并发提交的 reasoning replay；Grok 也缺少上游明确拒绝 opaque reasoning 后的受控恢复状态机。
3. **Kiro 本地 Prompt Cache 模拟不够忠实**：缺顶层 auto-caching、四断点上限、20-block lookback、混合 TTL 顺序和按条目 TTL 续期；估算 cache token 与上游总输入量未按比例校准，存在用量口径偏差。
4. **实现成熟度与证据等级不一致**：Cursor、Qoder、CodeBuddy 等本地实现很强，但 registry/conformance、文档和真实账号 receipt 尚未形成一致闭环。
5. **热路径与持久化结构已到拆分点**：`forwarder.rs`、`state.rs` 等巨型文件提高了重试边界和状态写入的回归风险；主要业务状态仍是整文件 JSON/JSONL，Kiro Prompt Cache 还会在请求路径同步写整份快照。

建议先做 P0 协议修复与用量守恒，再做 P1 真实验收和新模型能力，之后才开始 P2 热路径拆分与 SQLite 迁移。架构重构不得和协议行为改动混在同一个提交中。

## 2. 范围、证据与边界

### 2.1 推荐参考项目冻结点

| 类别 | `AGENTS.md` 推荐参考 | 本次读取基线 | 工作树说明 |
| --- | --- | --- | --- |
| Antigravity | `CLIProxyAPI`、`Antigravity-Manager` | `09a29bd345bc`、`85fb4fe68899` | 干净 |
| Claude | `CLIProxyAPI` | `09a29bd345bc` | 干净 |
| Codex | `CLIProxyAPI`、`Codex/codex2api` | `09a29bd345bc`、`3028f44ef1d6` | 干净 |
| Cursor | `OmniRoute` | `a3ca33fa6442` | 有 22 项未提交改动；只使用提交态证据 |
| Grok | `Grok/grok2api`、`sub2api` | `8913b53fe923`、`ab99d56e9626` | `sub2api` 有 6 项未提交改动；只使用提交态证据 |
| Kiro | `Kiro/kiro.rs` | `22d2c2d0695b` | 干净 |
| Qoder | `TokenRouter` | `3488b4a9208c` | 有 10 项未提交改动；只使用提交态证据 |
| CodeBuddy | `cli2api` | `4ca0e742ea65` | 有 2 项未提交改动；只使用提交态证据 |

这些仓库仅是一次性、只读差异证据，不得成为本仓库的构建依赖、运行时输入、同步源或 CI checkout。引用 commit 只证明“参考项目存在这种处理”，不能替代厂商证据和本仓库验收。

### 2.2 差距判定

- **确认缺失**：在目标代码、合同、fixture 和历史中均未找到等价实现，可直接进入设计与实现。
- **差分验证**：目标已有相邻能力，但没有覆盖参考项目暴露的精确失败形状；先冻结最小 fixture，只有红灯时才修改生产代码。
- **验收缺口**：实现和离线合同已存在，主要缺真实账号、状态闭环或脱敏 receipt，不应重写实现。
- **不采纳**：与 Server 产品边界冲突，或证据质量不足，不进入 backlog。

任何离线 fixture、mock server 或参考项目的 live 记录都不能把本仓库状态升级为 `live_verified`。真实验收必须由本仓库 harness 产生脱敏 receipt。

### 2.3 必须保持的产品不变量

- Share、Provider Surface 和一个明确 Account 固定绑定；重试只能留在同 Provider、同账号、同 rail、同身份代际，禁止账号池选号、跨账号轮换和跨 Provider fallback。
- 首个业务输出提交后禁止透明 replay；所有恢复必须在 pre-commit 且受统一总重试预算约束。
- 新状态写入继续走 `ServerStateInner` 域方法，不能在 `state.rs` 外直接修改内部 store；锁顺序保持 `config → providers → accounts → usage → shares → ui_settings → sessions → oauth_logins`。
- 不迁入桌面/Tauri、签到、运营自动化、商业计费、Key 分销、多租户账号调度或外部项目 UI。
- 外部返回的 token、opaque reasoning、tool 参数和错误体都按秘密处理；日志、指标和 receipt 不记录原文。

## 3. 目标项目基线

### 3.1 结构风险

| 文件 | 当前行数 | 风险 |
| --- | ---: | --- |
| `src/proxy/forwarder.rs` | 43,365 | Provider 分支、请求构造、HTTP/WS、重试、缓存、流式提交和 usage 耦合在一条热路径 |
| `src/state.rs` | 33,837 | 状态门面、事务、持久化、后台任务和大量 Provider 编排集中 |
| `src/clients/oauth/quota.rs` | 11,244 | 多 Provider quota 协议、解析和状态投影集中 |
| `src/proxy/transforms.rs` | 10,514 | 多协议双向转换共享一个文件，难以隔离 Provider 方言 |
| `src/proxy/adapters.rs` | 9,093 | 路由、模型、缓存注入和适配策略耦合 |
| `src/proxy/stream_transforms.rs` | 8,416 | 多种流生命周期共享状态机代码，终态改动容易横向回归 |

目前已有清晰的 `api / proxy / domain / clients / infra` 依赖边界，也已有不少 Provider 专属模块，因此增强方案应采用渐进式抽取，不做整仓重写。

### 3.2 存储与 I/O

- Provider、Account、Share 仍主要保存为 `providers.json`、`accounts.json`、`shares.json`；usage 使用 snapshot、JSONL journal 和 rollup。
- `rusqlite` 已在依赖中，并已用于 Router control store 和 Cursor 本地导入，但核心业务 store 尚未迁移。
- `src/proxy/kiro.rs::KiroPromptCache::flush_snapshot` 每次计算后克隆整张表，随后同步 `create_dir_all`、序列化并 `std::fs::write`；这是确认存在的请求路径阻塞和写放大。
- 现有数据目录独占锁、原子 JSON 写、凭据 XChaCha20-Poly1305 加密、备份 stage/validate 都是迁移时必须保留的安全属性。

### 3.3 当前 conformance 真值

| Driver | forward | test | discovery | 判断 |
| --- | --- | --- | --- | --- |
| Claude OAuth | `fixture_verified` | `live_pending` | `fixture_verified` | 实现强，真实订阅门禁未闭环 |
| Codex OAuth | `fixture_verified` | `fixture_verified` | `unsupported` | HTTP/WS/Images 强，仍有新 schema/model 差异 |
| Grok OAuth | `fixture_verified` | `live_pending` | `fixture_verified` | 能力面广，reasoning replay/recovery 缺失 |
| Qoder COSY | `fixture_verified` | `live_pending` | `fixture_verified` | 离线 oracle 强，三 rail 尚无真实 receipt |
| CodeBuddy OAuth | `live_pending` | `live_pending` | `live_pending` | 本地实现完成，但国内外真实闭环不足 |
| Kiro | `fixture_verified` | `unsupported` | `fixture_verified` | wire 强，Prompt Cache 模拟弱 |
| Cursor | `implemented` | `implemented` | `unsupported` | 实际已有局部目录能力，合同和证据状态漂移 |
| Antigravity / agy | `implemented` | `unsupported` | `fixture_verified` | 基本转发可用，多轮连续性和恢复能力不足 |

## 4. 八类 Provider 深入对比

### 4.1 Antigravity

#### 已有能力

目标已覆盖 managed OAuth Account、项目/tier/quota 探测、模型目录、Claude/Gemini/OpenAI 方言桥接、mixed tools、付费 tier endpoint 选择、Share/runtime/model cooldown，以及同账号的一次短 429/503 重试。`antigravity_oauth` 与 `agy_oauth` 保持独立身份标签，这是正确边界。

#### 参考领先点与差距

1. **确认缺失：reasoning replay。** `CLIProxyAPI/internal/cache/antigravity_reasoning_replay_cache.go` 已实现有界 TTL/LRU、conversation/model scope、缺失 tombstone、CAS snapshot、branch/revision、整链替换/删除和 KV 扩展；目标没有 Antigravity 专属 replay。参考的核心价值是并发写 fencing 和上下文漂移检测，不是 KV 产品本身。
2. **确认缺失：conversation compaction rail。** `CLIProxyAPI@70f45604` 在 Antigravity executor 中处理 Responses `compaction_trigger`、生成摘要并封装 capsule。目标只有 Codex overflow compaction，没有 Antigravity rail。参考代码使用固定字符串派生 AES key，不符合本项目秘密边界，不能照搬。
3. **差分验证：专属 schema sanitizer。** `CLIProxyAPI/internal/runtime/executor/antigravity_executor_request.go` 对 Gemini/Antigravity 方言做专门清理；目标主要复用通用 Gemini schema normalizer。需用真实拒绝样本判断是否应增加专属规则。
4. **差分验证：连接池生命周期。** `CLIProxyAPI@d5397905` 默认短连接并强化 transport cache，`@68dd99d5` 把 resolved pool settings 纳入 cache key。目标全局 reqwest client 默认为每 host 10 个 idle、TCP keepalive 60s，HTTP/2 keepalive 可选；尚未有 Antigravity 专属长流/中断矩阵。
5. **差分验证：session rollover 与 quota gate。** `Antigravity-Manager@85fb4fe68899` 使用 conversation-scoped `sessionId`，检测上游累计输入超过 1 Mi token 后升代；同时增加 zero-quota 持续锁、最大退避上限和临时 503 的标准 `Retry-After`。目标已记录 Antigravity model capacity evidence 和 cooldown，但没有等价的 session generation 证据。

#### 增强项

- **AG-01 / P0：实现 Antigravity reasoning replay。** 新增 Provider 专属模块，scope 至少包含 App、Provider id/revision、runtime fingerprint、Account id、auth/token generation、Share、签名用户、session、model family、upstream plane。只保存重放所需最小 opaque item；限制 entry 数、每 entry item 数、序列化字节、TTL；读取返回 generation snapshot，成功终态用 CAS 提交，400/上下文漂移只删除实际使用过的 snapshot。并行 tool calls、相同 call id、无 id call、乱序 result、编辑历史、过期和代际漂移必须有 fixture。
- **AG-02 / P1：补 sessionId 派生和超限升代。** 先冻结 Antigravity wrapper 中 `request.sessionId` 的 wire 位置和错误签名；稳定 ID 由账号和下游 conversation scope 派生，绝不能只按账号；仅在明确的累计上下文超限、pre-commit、同账号且预算允许时升代重试一次。并验证 count-tokens/search 等不应携带 session 的操作。
- **AG-03 / P1：补 zero-quota、Retry-After、schema 和 transport 差分套件。** 只有明确的全模型零容量及 reset 证据才能安装持续 gate；unknown/空 bucket 不得伪装成耗尽。503/429 输出保留安全且有上限的 Retry-After。分别测试短连接、连接复用、服务端 GOAWAY、长 SSE idle 和配置变更后的 client cache key，再决定是否加 Provider 级 transport policy。
- **AG-04 / P3：按证据门禁实现 compaction。** 只有真实协议确认支持后才开放。capsule 使用本仓库根密钥按用途派生的 versioned AEAD，associated data 绑定 Provider/Account/session/model；不使用参考项目固定 secret。摘要失败、解密失败、过期、模型漂移均 fail closed 或回到明确的 omission marker，不能悄悄跨账号重做。

### 4.2 Claude

#### 已有能力

目标已经实现 Claude Code 2.1.258 wire profile、CCH、prompt-derived billing suffix、动态 beta、Fable 5.1、1h/5m cache-control 全局治理、最多四个高价值断点、quota headers、同账号 401 恢复、严格流终态和多协议 usage 归一。这里不应重做已有缓存注入或泛化成另一套 Claude executor。

#### 参考领先点与差距

1. **确认存在失败窗口：native passthrough 的 leading system role。** `OmniRoute@a3ca33fa6442` 证明开启 `mid-conversation-system` 时，`messages[0]` 的 text-bearing system/developer 仍会被 Anthropic 400；修复只提升首个真实 user/assistant 之前的文本 system，保留真正中段 system 和 directive-only message。目标第三方 OpenAI→Anthropic 会把所有 system 收到顶层，但 confirmed-native passthrough 不做这一步。
2. **差分验证：subagent 1h TTL。** `CLIProxyAPI@6a73f396` 修复显式请求 1h 的 subagent 被误剥离 TTL/beta。目标能识别 native/helper 并能从 body 生成 `extended-cache-ttl`，但没有 subagent × probe/helper × 5m/1h 的专项矩阵。
3. **差分验证：Claude→OpenAI trailing usage。** `CLIProxyAPI@a59b1764` 跨 `message_start`/`message_delta` 聚合 usage，并在 `include_usage` 下输出 `choices: []` 尾块。目标已有 usage merge 和大量 include-usage fixture，但缺与该精确跨协议时序一一对应的 differential golden。

#### 增强项

- **CL-01 / P0：修复 leading text system。** 在 Claude OAuth native passthrough、CCH 最终签名前执行：只扫描 leading run；把 string 或 text block 转入 top-level `system`；directive-only `content: [] + output_config` 留给其既有位置规则；首个真实 turn 后的 system 原位保留。四组最低 fixture：已有 string system、已有 block system、directive/text 混排、普通 user-first no-op；再覆盖缓存断点和 CCH 重算。
- **CL-02 / P1：建立请求类别缓存矩阵。** 组合 main/subagent/probe/helper/count_tokens、客户端显式 1h/5m、body/header beta、native/third-party、stream/non-stream。预期由官方 CLI fixture 冻结；目标已正确时只补测试，禁止为“对齐参考”改生产逻辑。
- **CL-03 / P1：建立跨协议 usage 时序 golden。** 覆盖 input 在 `message_start`、output 在多个 `message_delta`、cache read/write 分散出现、显式零值、失败前局部 usage、`include_usage=false/true`。尾 usage chunk 必须位于 finish chunk 后、`[DONE]` 前，且不重复计费。
- **CL-04 / P2：把 structured output、sequential interleaved tools、helper request id 等参考差异纳入定期审计。** 每项先做 fixture 红灯，不建立泛化兼容层，也不复制 CLIProxyAPI 的账号池或 cloaking 策略。

### 4.3 Codex

#### 已有能力

目标已覆盖 Responses HTTP/WS、WS pool scope、compact endpoint、overflow auto-compact、`previous_response_id`、Images generation/edit、service tier/fast、模型级 Share cooldown、工具 schema 递归遍历、usage、session headers 和同账号 refresh。`openai_capacity_shed.rs` 已能识别 `server_is_overloaded`/`slow_down`，区分业务输出并处理 pre-commit capacity retry，因此“完全缺 bootstrap overload”不是准确结论。

#### 参考领先点与差距

1. **确认缺失：Unicode property regex 清理。** `CLIProxyAPI@e56abd56` 删除 Codex 不支持的 schema `pattern` 中 `\p{}`/`\P{}`；`@37ce368c` 进一步检查 `patternProperties` 的键和 JSON `\u005c` 绕过。目标 `src/proxy/tool_schema.rs` 只递归访问这些节点，不清理不兼容表达式。
2. **确认缺失：大型纯 const union 简化。** CLIProxyAPI 的 `codex_tool_schema.go` 只在 oneOf/anyOf 全部是唯一纯 const、且语义可证明等价时转换为 enum，避免大 schema 触发上游 abort。目标保留所有 union。
3. **确认缺失：GPT Image 2.5。** `CLIProxyAPI@d1a024e9` 与 `codex2api@f5220891` 已覆盖 `gpt-image-2.5`、`gpt-image-2.5-flare`、`gpt-image-2.5-sunburst` 的 generation/edit 路由、模型前缀/尺寸别名和目录。目标当前只有较早的 Codex image family，不能只把新名字加入静态列表后宣称支持。
4. **差分验证：Responses 错误与序列语义。** `CLIProxyAPI@25913086` 保留 nested error detail 和 Responses sequence number；目标已有严格 SSE/WS 终态，但尚缺同一异常帧序列的逐字段 golden。`@3ae9093d` 的 bootstrap capacity 情形目标已经由 `openai_capacity_shed.rs` 覆盖，应补回归而不是另造 retry。
5. **差分验证：WS prewarm。** `CLIProxyAPI@bd03aabc` 保留 prewarm input，并接受具名 tool output 的合法序列。目标已有 WS pool 和 HTTP fallback，但没有同等的 prewarm 生命周期证据；这属于性能/兼容优化，不是 P0 正确性缺口。

#### 增强项

- **CX-01 / P0：实现 Codex Unicode schema sanitizer。** 在最终工具 schema 序列化前、有界递归清理 `pattern` 和 `patternProperties` key 中的 `\p{...}`/`\P{...}`，同时识别 JSON 解码后的反斜杠和 `\u005c` 绕过；不修改 description、普通字符串或 ECMA 兼容 pattern。对深度、节点数、总字节设限，并用 nested array/object、escaped key、恶意深嵌套和 mutation/property fixture 验收。
- **CX-02 / P1：安全简化大型纯 const union。** 仅当 `oneOf`/`anyOf` 每个分支都是无额外约束、类型兼容且值唯一的纯 `const` 时改写为 `enum`；保留 description/default/nullability 和父级约束。混合 type、`$ref`、object constraint、重复值或不可证明等价时保持原样。以 wire 大小下降、语义等价和上游不再 abort 三项共同验收。
- **CX-03 / P1：证据门禁接入 GPT Image 2.5。** 分别验证三个模型的 generations、multipart edits、model normalization、尺寸/质量参数、响应 usage、错误和 quota/cooldown；registry、目录和 UI 只发布真实账号确认可用的 variant。不得从 `gpt-image-2.5` 成功外推 flare/sunburst entitlement。
- **CX-04 / P1：补 Responses HTTP/SSE/WS differential golden。** 冻结 nested error、sequence number、named tool output、首帧/尾帧、bootstrap overload 和 WS→HTTP pre-commit fallback。目标现有行为正确的部分只加 fixture；任何 fallback 继续共用总 attempt budget，post-commit 禁止重放。
- **CX-05 / P3：评估 WS prewarm。** 只有基准显示稳定改善 TTFB 且 upstream receipt 证明协议允许时才实现；prewarm pool key 必须包含 Provider、Account、auth/token generation、runtime、model/feature profile，取消和代际漂移立即销毁，不得跨账号借用连接。
- **CX-06 / P3：生图 quota/cache 性能治理。** 在真实 2.5 证据之后评估 multipart 内存峰值、图片响应体上限、quota 刷新和 bounded cache；图片/prompt 不进入日志或 receipt，商业计价逻辑不从 codex2api 迁入。

### 4.4 Cursor

#### 已有能力

目标的 `special.cursor` 已同时覆盖 OAuth DeepControl 与 API-key exchange，两条 rail 均有独立 credential scope；具备 ServerConfig protobuf 校验、AgentService/open-sse 解码、Claude/Codex/Gemini 三 Surface、tools、reasoning、图片、park/resume、MCP wrapper、绝对 deadline、同绑定 pre-commit 401 恢复，以及 exact-scope live catalog 与 bounded last-known-good。现有专项 fixture 的覆盖面不弱于 OmniRoute，不能再引入第二套 Cursor executor。

#### 参考领先点与差距

1. **验收缺口，而非实现缺口。** OmniRoute 的 Cursor executor、session manager、protobuf codec 和 SDK 集成提供了真实环境的交叉参考；目标已有对应 wire 实现和大量离线 fixture，但 OAuth 与 API-key 的真实 receipt 尚未分别闭环。
2. **确认存在合同状态漂移。** 目标代码和历史计划已具备局部模型目录、forward/test fixture，但当前 registry 仍是 `driverContractRevision: 3`、forward/test=`implemented`、discovery=`unsupported`；仓库文档另有 revision 4、`fixture_verified` 的叙述。必须先决定“目录仅为内部 runtime 能力”还是公开 discovery operation，再由生成源统一表达。
3. **差分验证：SDK/protobuf 漂移。** OmniRoute 提供 open-sse protobuf 和 session 行为样本，但其提交态之外有本地改动，本次不把未提交内容当证据。后续只冻结经官方/真实账号复核的未知字段、duplicate URL、完整模型 ID、park/resume 和 EOF 样本。

#### 增强项

- **CUR-01 / P1：收敛 Cursor 合同真值。** 盘点生产入口、fixture、registry、coverage、UI 和 `docs/provider/cursor.md`；如果 discovery 只是内部 catalog resolution，则保持 operation unsupported 并修正文案；如果公开端点已完整实现，则补 contract test 后提升状态。revision 和 conformance 只能由同一生成源变更，禁止为了消除 diff 虚报 `live_verified`。
- **CUR-02 / P1：完成双 rail 真实验收。** OAuth 与 API-key 分开覆盖 fresh/empty/stale catalog、完整 `*-fast` ID、三 Surface stream/non-stream、tools/images、park/resume、401、deadline 和身份代际漂移；receipt 分开记录 rail 与脱敏 scope digest，任一 rail 不借另一 rail 的成功状态。
- **CUR-03 / P2：建立 protobuf/SDK 周期差分。** 把经审计的 OmniRoute/官方样本复制为本仓库自包含 fixture，验证未知字段保留、重复/冲突字段 fail closed、frame/EOF 和 deadline；外部 Node/Electron/SQLite/session UI 不进入运行时或 CI。

### 4.5 Grok

#### 已有能力

目标已覆盖 xAI OAuth、固定 CLI identity/version gate、动态目录及 ETag/304、exact-scope stale、Responses/Chat/Claude/Gemini 转换、HTTP/SSE/WS、search、图片/编辑/视频异步任务、capability evidence、严格流终态、稳定 Chat `created`、用量、模型 cooldown，以及同账号 401 和 pre-commit WS→HTTP fallback。媒体 task ownership 已绑定 Share、用户 namespace、Provider、Account、credential generations 和 upstream plane。

#### 参考领先点与差距

1. **确认缺失：conversation reasoning replay。** `grok2api@8641a782`、`@ca392e68` 用 conversation cache 恢复多轮 tool-call 的 reasoning context，`@7d1b4246` 又修复重放容量计算溢出。目标没有 Grok 专属 opaque reasoning cache，因此无状态客户端在下一轮只回传 tool result 时可能失去上游要求的 reasoning item。
2. **确认缺失：opaque reasoning 拒绝后的受控恢复。** `grok2api@3de758e7` 能在 Responses history 中检测上游明确的 missing/invalid reasoning 错误，去除本次注入的失效块并单次自愈。目标没有该状态机；不能把任意 400 都当成可删除 reasoning 后重试。
3. **确认缺失：Grok Build 根 union。** `@22ac653a`、`@72a3a347`、`@5d19ccff`、`@e5285ebe` 处理工具参数根部 `anyOf`/`oneOf`、非 object 分支和深层 local refs。目标通用 sanitizer 不具备这一 Provider 方言，复杂 MCP schema 可能被 Grok Build 拒绝。
4. **差分验证：错误/compaction/传输边界。** 参考实现还有 remote compaction 等行为，但目标已有严格 commit boundary、capacity/cooldown 和 WS fallback。必须以真实错误 code/body shape 证明差距，不能把网页 rail 或 sub2api 的账号调度推入 OAuth rail。

#### 增强项

- **GR-01 / P0：实现 Grok reasoning replay。** 使用 Provider 专属 value schema和共享的 typed scope/CAS 基础件；scope 包含 App、Provider revision/runtime、Account、auth/token generation、Share、签名用户、session/turn、model family 和 rail。容量计算全部 checked/saturating，有 entry/item/bytes/TTL 上限；只在权威成功终态 CAS 提交，乱序/重复 tool call、分支编辑、并发 turn、过期和代际漂移必须 fail closed。
- **GR-02 / P0：实现明确错误驱动的 reasoning recovery。** 仅匹配冻结 fixture/真实 receipt 中的 missing、invalid 或 rejected opaque reasoning；仅可删除“本 attempt 实际从 cache 注入”的 snapshot/tombstone，并在同账号、pre-commit、总预算内重建请求一次。客户端原带 reasoning、普通 schema 400、auth/capacity 错误和 post-commit 流不得走此恢复。
- **GR-03 / P0：增加 Grok Build root-union adapter。** 在通用 sanitizer 后、Grok wire 序列化前有界解析 local `$ref`，只保留可证明为 object 的根分支并合并共同约束；无法无损表达、循环 ref、超深/超大和全非 object 时明确拒绝，不能静默变成 `{}` 或放宽 schema。用四个参考 commit 的 failure shape 建 golden。
- **GR-04 / P1：补多轮、错误与真实验收矩阵。** 覆盖 reasoning replay/reject、parallel tools、HTTP/SSE/WS、version gate、429/cooldown、search/media ownership 和 catalog stale；receipt 仍固定唯一 Share/Account，并分别记录推理和媒体 capability，缺真实数据时保持 `live_pending`。
- **GR-05 / P3：证据门禁评估 remote compaction。** 只有 OAuth rail 的真实协议证据证明可用才设计，scope、AEAD、TTL、失败语义与 Antigravity compaction 同等级；不采用 Grok Web cookie、跨账号 cache 或 sub2api 商业路由。

### 4.6 Kiro

#### 已有能力

目标已有独立 Kiro OAuth/Account/profile/region authority、模型目录、Claude/Codex 转换、CodeWhisperer EventStream CRC/终态、tool/image bounds、同账号 401、generation-scoped runtime/cache key 和 prompt-cache 本地估算。Kiro 与 Amazon Q 已被建模为不同 Provider/credential/endpoint，不能按 OmniRoute 的旧 alias 方式重新合并。

#### 参考领先点与差距

1. **确认缺失：完整 Prompt Caching 语义。** `kiro.rs@f2cc574` 实现顶层 `MessagesRequest.cache_control` auto-caching、最多四断点、20-block lookback（连续 tool_use/tool_result 分组）、默认 5m/显式 1h、1h 必须先于 5m，以及按 entry 自身 TTL 滑动续期。目标只从 tool/system/message/block 显式标记生成线性 segments，没有这些限制与回溯规则。
2. **确认缺失：真实 total 口径下的 usage 守恒。** `@19b7f4b`、`@47633a4` 把缓存覆盖比例映射到上游 `contextUsage`/count total，保证 `input + cache_creation + cache_read == total`。目标直接把本地字符估算的 read/creation 从上游 input 中相减，两个估算器尺度不同，可能夹断为零或扭曲三项比例。
3. **确认存在热路径同步 I/O。** 目标 `KiroPromptCache::flush_snapshot` 每次请求克隆整表并同步写完整 JSON。参考的本地/Redis abstraction 说明可把存储移出计算路径，但 Redis 与 session affinity 不是单机 Server 的默认需求。
4. **验收缺口。** 目标 wire fixture 很强，真实 Builder ID/IdC/Social/API-key、跨 region 目录、stream/tool/image/quota/401 receipt 仍缺；参考项目的 UI trace 或其他账号流量不能替代。

#### 增强项

- **KI-01 / P0：重做 Kiro Prompt Cache 语义层。** 支持 top-level auto 与显式 breakpoint；统一编号并限制四个；实现 20-block lookback、tool_use/tool_result group、混合 TTL 顺序校验和 entry 自身 TTL 续期。cache key 保持 Provider/Account/generation/Share/user/session/model/region 隔离；非法声明采取明确 no-cache/fail-closed 策略并观测原因，不制造虚假命中。
- **KI-02 / P0：按比例拆分 cache usage。** 本地估算只计算 `covered_est / prompt_total_est` 和命中比例，再映射到上游 authoritative total；采用确定性舍入并把余数归入明确字段，使三项非负且严格守恒。上游已直接提供 cache usage 时优先保留真值，本地模拟必须标 source，不能宣称实际降低 Kiro 推理成本。
- **KI-03 / P1：移除请求路径同步整表写。** 先接 bounded async writer、dirty generation、合并/退避和 shutdown flush；写失败不阻塞响应且不伪造 durable。随后把该表作为 SQLite 首批迁移域，用 transaction/CAS 和容量清理替代 JSON snapshot；崩溃、磁盘满、并发更新和重启 TTL 必须验收。
- **KI-04 / P1：补缓存与真实 rail 验收。** differential 覆盖 auto/explicit/no-cache、1h/5m、四断点/超限、20-block 边界、最新 user guardrail、上游 usage 优先级；真实 receipt 按 auth kind 和 region 分开，未提供凭据时维持现有 conformance。
- **KI-05 / P3：按部署需求评估共享 cache。** 只有明确支持多副本且本地 SQLite 不足时才引入 remote store；要求短超时、熔断、namespace/version、TLS/secret 管理和本地降级。不得迁入参考的账号 affinity/调度，也不得让 cache 命中改变固定 Account binding。

### 4.7 Qoder

#### 已有能力

目标以官方 Qoder CLI 1.1.32 的自包含 oracle 为主证据，已独立实现 Global OAuth、Global PAT、CN OAuth 三 rail：Device lifecycle/refresh、站点化 machine identity、COSY signing/session、目录/effort/context capability、Claude/Codex/Gemini、tools/reasoning/usage、严格 terminal+EOF、quota 和同账号 pre-commit 401。相对 TokenRouter，目标的固定绑定、三 rail 隔离、oracle mutation 和 receipt 脱敏边界更符合本项目，不需要重写 executor。

#### 参考领先点与差距

1. **主要是合同状态收敛。** TokenRouter 的 upstream interface/account maintenance 可继续交叉核对 endpoint、job token、quota 和错误分类；但当前目标历史文档称 driver revision 2，registry 仍为 revision 1。实现、oracle、生成 coverage 与 registry 需要一次统一审计。
2. **主要是 live gate。** 三 rail 的 loopback harness 已存在，但 Global OAuth、Global PAT、CN OAuth 的真实 receipt 均未提供，所以 test 仍为 `live_pending`。任何一条 rail 的成功不能外推另一站点或 credential kind。
3. **周期漂移风险。** 目前 oracle 固定 npm bundle hash、精确 header/path/body/schema 和 verification 计数，这是优势；升级 CLI 时若 fixture 与实现同改而没有独立来源复核，仍可能假绿。TokenRouter 只能作第二来源，不能覆盖官方 oracle。

#### 增强项

- **QD-01 / P1：统一 Qoder contract revision 和状态。** 从 oracle verification、生产入口、registry、coverage/UI、provider 文档生成一张可审计映射；确认 revision 2 的条件全部在当前 HEAD 后再升级，否则修正文档。operation=`supported` 与 conformance 等级分开表达，不以 implemented/fixture 状态冒充 live。
- **QD-02 / P1：关闭三 rail 真实验收。** 分别运行 login/exchange、refresh rotation、fresh/empty catalog、quota、三 Surface stream/non-stream/tool/usage、pre-commit 401、terminal+EOF 和 decoy zero-request；receipt 绑定 site、rail、Provider/Account generations 与 wire digest。缺任一凭据只报告该 rail `live_pending`。
- **QD-03 / P2：建立 CLI 升级审计流程。** 新版本先冻结包 integrity/bundle digest，独立提取稳定 wire，再更新 oracle，最后才允许改 Rust；mutation 必须证明 endpoint/header/body/signature/identity 的单边漂移会红灯。TokenRouter 的商业计费、Key 管理、账号维护调度不纳入。

### 4.8 CodeBuddy

#### 已有能力

目标已实现 Intl/CN 固定站点、cookie-bound OAuth、独立 flow jar/lease/TTL、`site + uid + enterpriseId` 身份闭合、rotation-safe refresh receipt/CAS、约 24h session refresh 与抖动、`12153` needs-relogin 终态、`/v3/config` exact-scope 目录、三 Surface、顶层 reasoning、tools/usage、严格 `[DONE]`+EOF、quota 白名单和同账号 pre-commit 401。具名 `tool_choice` 已有校验，不能重复设计。Intl/CN、个人/企业和多模态的未验证边界仍须 fail closed。

#### 参考领先点与差距

1. **确认缺失：空 message 清理。** `cli2api@e5893f0` 删除 content 缺失/null/空白/空数组且无 tool_calls 的 message，保留承载 tool_calls 的空 content，并在清理后确保非空 leading system；目标尚缺等价专项处理，上游会以 `11151` 拒绝。
2. **确认缺失：空 streaming delta 清理。** `cli2api@32aa108` 去除上游稠密空 `content`、`reasoning_content`、`refusal`、空 `tool_calls` 和 dummy `function_call`，否则客户端收到大量空 thinking/content 事件。清理必须保留 finish、usage、真实 tool delta 和合法首 role。
3. **确认缺失：空 tools/tool_choice 组合。** 参考的 `PrepareBody` 删除 null/空 tools，并同步删除失去依附的 tool_choice；目标具名 choice 校验虽已正确，但没有冻结 empty/null/none 的 wire 矩阵。
4. **验收缺口。** 目标本地实现总体已强于 cli2api，registry 诚实保持三项 `live_pending`；Intl 与 CN 仍各缺本仓库真实订阅 receipt，企业和 image/video 没有证据时不得从参考项目开放。

#### 增强项

- **CB-01 / P0：规范化空消息。** 在 CodeBuddy canonical Chat payload 完成后、leading system 注入前，删除缺失/null/空白/空数组 content 且没有有效 tool_calls 的条目；保留 assistant tool call、tool result 等协议必要空 content。若清理后为空，注入固定非空最小 system；不改写普通 prompt。以 `11151` failure fixture、三 Surface 和 tool history 验收。
- **CB-02 / P0：抑制语义为空的 delta。** 在 CodeBuddy decoder 内字段级删除空 string/array/object 和 dummy function_call；整个 chunk 仅在没有 role、finish、usage、error、tool/reasoning/content 语义时丢弃。role 最多输出一次，分块 tool arguments 原样累计，终态+EOF 规则不变。
- **CB-03 / P0：收敛 tools/tool_choice 空值。** null/`[]` tools 不发往上游，并删除对应 `tool_choice`; `none` 按冻结 wire 同步禁用 tools；具名 function 继续使用现有 validator，名字不存在、空名或格式错误在发网前拒绝。覆盖 absent/null/empty/none/auto/required/named 组合，不能因清理放宽声明。
- **CB-04 / P1：完成双站真实验收与状态提升。** Intl/CN 分别验证 OAuth、24h refresh jitter、12153、目录、billing、三 Surface、empty payload、strict EOF 和 same-account 401；receipt 不含 cookie/token/prompt/uid 原值。个人站成功不开放企业或多模态，也不迁入每日签到、账号池、domain fallback 或 prompt rewrite。

## 5. 横切增强设计

### 5.1 统一 attempt/recovery 状态机

新增明确的 `AttemptBudget`/`CommitGuard`，由 Provider executor 使用，而不是每个分支各自累加布尔字段。至少表达：

- 当前 Provider/Account/rail/auth generation/token generation snapshot；
- auth、capacity、body compatibility、reasoning recovery、session rollover、WS→HTTP 各自阶段；
- 总 attempt 上限和每类上限；
- 是否已提交下游业务输出；
- 本次读取的 cache snapshot，只有命中该 snapshot 的请求才有权删除/替换；
- retry reason、delay source 和最终 decision 的低基数观测字段。

验收要求：任意组合都不能突破总上限；credential generation 漂移立即终止；首个业务输出后所有透明动作关闭；取消信号贯穿 refresh、backoff、network 和 cache wait。

### 5.2 Provider executor 渐进拆分

建议落点：

```text
src/proxy/execution/
  context.rs          # 固定绑定、AttemptBudget、CommitGuard
  transport.rs        # HTTP/SSE/WS 发出与 deadline
  terminal.rs         # 统一 pre-commit/committed 判定
  usage.rs            # 每 attempt 与最终 usage 合并
  recovery.rs         # 通用阶段框架，不含 Provider 方言

src/proxy/providers/
  antigravity/
  claude/
  codex/
  cursor/
  grok/
  kiro/
  qoder/
  codebuddy/
```

抽取顺序是“复制现有行为到模块并保持 golden 全绿 → 切换单个 Provider → 删除旧分支”，每个提交只移动一个关注点。Provider 方言、endpoint 和 retry classifier 留在 Provider 模块；共享层不能通过巨型 enum 再造一个 `forwarder.rs`。

完成标准：`forwarder.rs` 只负责入口编排和 dispatch；新增 Provider 不需要修改中央重试循环的多个远距离分支；所有现有 wire golden 字节级不变。文件行数不是单独 KPI，耦合和可验证边界才是。

### 5.3 `state.rs` 与 repository 边界

保留 `ServerStateInner` 作为跨域编排门面，但把实现下沉到：

- accounts credential lifecycle/recovery；
- provider runtime snapshot；
- share mutation/validation；
- usage append/query/compaction；
- cache/session ephemeral state；
- background schedulers。

域方法继续负责锁顺序和持久化，不把内部 `RwLock` 或数据库连接暴露给 proxy。先为现有 JSON store 建 repository trait 和 transaction contract，再换 SQLite backend，避免同时改 API、业务规则和存储。

### 5.4 SQLite 迁移

#### 目标

- 用 schema version + migration ledger 管理 providers/accounts/shares/usage 和适合持久化的短期 cache；
- 使用事务保证跨对象引用和 generation CAS；
- 消除整文件重写、JSONL 压实窗口和 Kiro 请求路径同步写；
- 保留字段级凭据加密，不把“SQLite 文件”误当成加密边界。

#### 分阶段方案

1. **STORE-01 / P2：定义 schema 和崩溃模型。** 冻结主键、foreign key、revision/generation、时间单位、opaque JSON 扩展列、usage 索引和保留策略；启用 foreign keys。WAL、synchronous、busy timeout 和 checkpoint 参数通过崩溃/性能测试决定，不能照搬外部默认值。
2. **STORE-02 / P2：实现 SQLite repository 与 shadow import。** 在数据目录独占锁下读取旧 JSON/JSONL，校验引用和 digest，写入临时 DB transaction，再执行逐表计数、关键字段 hash、Provider/Share graph 和凭据解密抽样校验。此时旧文件仍是权威。
3. **STORE-03 / P2：权威切换。** 用小型 migration marker 原子记录 `prepared → committed`；启动时可 roll forward。切换后旧 JSON 移入带时间戳的只读 migration backup，不做长期双写，避免 split-brain。
4. **STORE-04 / P2：备份/恢复与回滚。** 备份使用 SQLite online backup 或一致性 transaction snapshot，并纳入现有 manifest/stage/validate。至少保留一个发布窗口的离线 DB→legacy export 或版本回退工具；回滚前必须停服并持有数据目录锁。
5. **STORE-05 / P2：故障注入。** 覆盖磁盘满、rename/commit 失败、进程在每个 marker 状态退出、WAL 损坏、旧 JSON 损坏、重复迁移、降级二进制打开新目录、备份期间写入和恢复后 generation CAS。

建议先迁 usage/Kiro cache 这类高写入域，再迁 providers/accounts/shares；账号密文 envelope 原样保存，最后才考虑数据模型升级。

### 5.5 Cache 基础件

为 AG-01、GR-01、Kiro 和现有 previous-response cache 提供小型内部基础件，而不是一个懂所有 Provider 的通用缓存：

- typed scope key 和 domain-separated digest；
- entry/count/bytes/TTL 上限；
- LRU/expiry；
- generation snapshot + CAS replace/delete；
- sensitive value 不实现 Debug/Serialize-to-log；
- hit/miss/expired/rejected/conflict 指标无 tenant label；
- 可选 persistence trait，默认进程内。

每个 Provider 仍拥有自己的 value schema、有效性规则和提交时机。

### 5.6 Conformance 与真实 receipt

建立单向状态机：

```text
unsupported → planned → implemented → fixture_verified → live_verified
```

`live_pending` 是 gate/附加状态，不是成功等级。生成脚本应拒绝倒置组合，例如 operation=`unsupported` 但 UI 宣称 discovery 可用，或没有 receipt 却写 `live_verified`。

脱敏 receipt 最少包含：Provider/rail/site、仓库 commit、harness 版本、UTC 时间、模型、Surface、stream/non-stream、请求形状标签、HTTP/协议终态、usage presence、refresh/retry/cooldown 结果、脱敏 body hash 和结果。禁止保存 Authorization、Cookie、token、opaque reasoning、prompt、图片、真实邮箱/uid、完整上游错误体。

### 5.7 可观测性与性能

- 为 attempt 记录 stage、provider family、transport、pre/post commit、retry decision、delay source、terminal kind、usage source；不得用 Account/Share/user/session 作 metrics label。
- 请求日志只保存有界、脱敏的错误 code/class；原始错误仅在内存中用于当前响应清理。
- 为 cache 提供 entry/bytes、hit/miss/conflict/eviction；为 SQLite writer 提供 queue depth、batch、commit latency、drop/failure；为 stream 提供 TTFB、first-business-frame、idle、terminal。
- 建立基线 benchmark：无工具短请求、大工具 schema、100 个并行 tool calls、长 SSE、Kiro cache 4k entries、usage 写入 burst。验收以相同 fixture 的基线无显著回退和无 event-loop blocking 为准，不用任意吞吐数字掩盖协议错误。

## 6. 分阶段实施顺序

### Phase 0：冻结基线与差分 harness（P0，所有改动前）

| ID | 工作 | 退出条件 |
| --- | --- | --- |
| BASE-01 | 把本文确认的失败形状转为本仓库最小 fixture，不在测试时读取外部仓库 | fixture 有来源 commit/path、输入和预期，当前应失败的确实失败 |
| BASE-02 | 建立 HTTP/SSE/WS differential runner 和敏感字段扫描 | 同一 canonical 输入可比较 wire/terminal/usage；产物无秘密 |
| BASE-03 | 冻结现有八类 Provider golden 和性能基线 | 重构前后的字节、错误类别、attempt 数和 usage 可比较 |

### Phase 1：协议与账目正确性（P0）

建议顺序：

1. `CX-01` Codex Unicode schema sanitizer；
2. `GR-03` Grok root union adapter；
3. `CB-01`、`CB-02`、`CB-03` CodeBuddy 空载荷处理；
4. `CL-01` Claude leading system hoist；
5. `KI-01`、`KI-02` Kiro 缓存语义与 usage 守恒；
6. cache typed scope/CAS 最小基础件；
7. `AG-01`、`GR-01`、`GR-02` reasoning replay/recovery。

Phase 1 总退出条件：所有新增 failure fixture 转绿；property/mutation/overflow 测试通过；所有恢复都满足同账号、pre-commit、总预算和 generation fence；usage 守恒；现有 Provider golden 无非预期变化。

### Phase 2：可用性、新能力与真实验收（P1）

- `AG-02`、`AG-03`；
- `CL-02`、`CL-03`；
- `CX-02`、`CX-03`、`CX-04`；
- `CUR-01`、`CUR-02`；
- `GR-04`；
- `KI-03`、`KI-04`；
- `QD-01`、`QD-02`；
- `CB-04`；
- 统一 conformance/receipt gate。

Phase 2 退出条件：每个宣称 supported 的 operation 有 fixture 证据；需要 live 的 rail 有本仓库 receipt；没有真实凭据的项目仍诚实保留 `live_pending`；GPT Image 2.5 只公开真实证明的 variant。

### Phase 3：架构与存储（P2）

1. 引入统一 AttemptBudget/CommitGuard，但保持行为 golden 不变；
2. 按 Provider 渐进抽取 executor、transport、terminal、usage；
3. 拆分 `state.rs` 实现并建立 repository contract；
4. 执行 `STORE-01` 至 `STORE-05`；
5. 完成 `CUR-03`、`QD-03`、`CL-04` 和观测/性能基线。

Phase 3 退出条件：协议和重构提交分离；旧数据可无损迁移、崩溃恢复和回滚；热路径无同步整表写；备份/恢复覆盖 SQLite；依赖方向和锁顺序审计通过。

### Phase 4：证据驱动的可选能力（P3）

- `AG-04` Antigravity compaction；
- `GR-05` Grok remote compaction；
- `CX-05` WS prewarm、`CX-06` image quota/cache 性能；
- `KI-05` 多副本共享 cache。

这些项目没有新协议证据时不得提前；“参考项目已实现”不是启用理由。

## 7. 测试与发布门禁

每个实现 PR 至少执行与改动范围相称的以下门禁：

```bash
cargo fmt --check
cargo check
cargo test
node scripts/audit/audit-server-provider-contract.mjs
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
node scripts/audit/audit-docs-index.mjs
scripts/smoke/smoke-local.sh
RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh
```

额外要求：

- schema 变更跑深度/节点/字节/Unicode/mutation/property 测试；
- replay/cache 变更跑并发、CAS、代际漂移、TTL、容量和取消测试；
- stream 变更跑分块边界、首业务输出、idle、terminal、EOF、重复 terminal、错误后数据和 usage 时序；
- SQLite 变更跑迁移矩阵、故障注入、备份恢复和旧版本回滚；
- live harness 只从私密环境读取凭据，不把凭据放进命令行、日志、fixture 或 receipt；
- 真实输入缺失时只跑离线 readiness，禁止改写为 live success。

## 8. 明确不纳入计划的参考能力

- 任意多账号选号、权重轮询、跨账号 fallback、跨站点 fallback、账号池 sticky routing；
- TokenRouter/codex2api/sub2api 的渠道计费、Key 分销、商业配额和管理后台产品逻辑；
- OmniRoute 的 Next.js/Electron/桌面/UI/SQLite 应用框架；
- Antigravity-Manager 的 Tauri 桌面能力；
- CodeBuddy 每日签到或其他改变账号运营状态的自动化；
- 为“兼容”而改写用户 prompt、屏蔽模板或隐藏真实上游错误；
- 未经本仓库真实证据开放 CodeBuddy 企业/图像、Grok remote compaction、Antigravity compaction 或 Kiro 多副本 Redis；
- 使用固定常量派生 capsule 密钥、把 opaque reasoning 写日志、或把外部仓库加入 CI/运行时。

## 9. 参考证据索引

### Antigravity

- `CLIProxyAPI/internal/cache/antigravity_reasoning_replay_cache.go`
- `CLIProxyAPI/internal/runtime/executor/helps/antigravity_compaction.go`
- `CLIProxyAPI@70f45604`、`@d5397905`、`@68dd99d5`
- `Antigravity/Antigravity-Manager/src-tauri/src/proxy/common/session.rs`
- `Antigravity/Antigravity-Manager@85fb4fe68899` 的 session rollover、zero-quota lock、max backoff、Retry-After 变更

### Claude

- `CLIProxyAPI@6a73f396`：subagent 显式 1h TTL/beta
- `CLIProxyAPI@a59b1764`：Claude stream usage 聚合与 trailing usage chunk
- `OmniRoute@a3ca33fa6442`：leading text system hoist

### Codex

- `CLIProxyAPI@e56abd56`、`@37ce368c`：Unicode property escape 与 `patternProperties`
- `CLIProxyAPI/internal/runtime/executor/helps/codex_tool_schema.go`：纯 const union → enum
- `CLIProxyAPI@d1a024e9`、`Codex/codex2api@f5220891`：GPT Image 2.5
- `CLIProxyAPI@3ae9093d`、`@25913086`、`@bd03aabc`：bootstrap overload、nested error/sequence、WS prewarm

### Cursor

- `OmniRoute` 的 Cursor/open-sse protobuf、stream 和 credential scope fixture；只参考提交态 `a3ca33fa6442` 之前的 Cursor 相关历史

### Grok

- `Grok/grok2api/backend/internal/infra/provider/conversation/reasoning_cache.go`
- `Grok/grok2api/backend/internal/infra/provider/cli/responses_reasoning_recovery.go`
- `Grok/grok2api/backend/internal/infra/provider/cli/responses_tool_declarations.go`
- `Grok/grok2api@8641a782`、`@ca392e68`、`@3de758e7`、`@22ac653a`、`@72a3a347`、`@5d19ccff`、`@e5285ebe`、`@7d1b4246`

### Kiro

- `Kiro/kiro.rs/src/anthropic/cache_metering.rs`
- `Kiro/kiro.rs/src/anthropic/stream.rs`
- `Kiro/kiro.rs@f2cc574`、`@19b7f4b`、`@47633a4`

### Qoder

- `TokenRouter/docs/interfaces/qoder_upstream.md`
- `TokenRouter/docs/operations/account_maintenance.md`
- 本仓库 `assets/contract/qoder-cli-oracle.json` 仍是高于兼容参考的官方冻结证据

### CodeBuddy

- `cli2api@e5893f0`：空 WorkBuddy messages
- `cli2api@32aa108`：空 streaming delta
- 本仓库 `docs/provider/codebuddy-oauth.md` 与 `PROTOCOL_EVIDENCE.md` 是站点、身份、session refresh 和不支持边界的权威来源

## 10. 完成定义

本计划只有同时满足以下条件才算完成：

1. P0 确认缺失项已实现并通过故障/并发/边界测试；
2. 所有“差分验证”项都有明确结论，未复现的问题只保留测试，不为对齐而改代码；
3. 八类 Provider 的 registry、coverage、UI 和真实 receipt 状态一致；
4. 真实凭据缺失的能力仍标记 `live_pending`；
5. Provider 固定绑定、同账号恢复和 post-commit 禁止 replay 的不变量无回归；
6. 核心热路径模块化，Kiro 无同步整表写，SQLite 迁移可 crash-recover 且可回滚；
7. 外部参考代码没有进入构建、测试、发布或运行时依赖。
