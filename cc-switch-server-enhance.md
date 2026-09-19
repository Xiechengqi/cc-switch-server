# cc-switch-server 反代增强增量分析与实施计划

> 文档性质：八类反代的增量差异分析与实施路线图，不是架构或协议真值。架构以 docs/architecture/overview.md 为准，Provider 身份与能力以 assets/contract/provider-registry.json 为准，wire 证据以 PROTOCOL_EVIDENCE.md、厂商材料和本仓库冻结 fixture 为准。
>
> 分析日期：2026-09-18，实施状态更新至 2026-09-19。分析起点：`origin/main@7c9ef35`；Provider 生产差分起点为 `4712ca063930fea507910c37749595c8d6acc073`。首轮生产行为与协议证据冻结到 `db65188`，验证门禁收口到 `248e7c2`。后续已按 Provider 完成 Antigravity（`e5bfc34`）、Claude（`6b0a0fe`）、Codex（`069f3ef`）、Cursor（`8f72bb9`）、Grok（`269850a`）、Kiro（`1124082`）、Qoder（`2cc8a01`）与 CodeBuddy（`d81056c`）的首批 CORE-N1/LIVE-N1 切片，并完成八类 EVID-N1 迁移；其余横切项仍按本文顺序推进。
>
> Provider 协议差异定位从 ae7fc88 开始；提交前 main 新增 6799870（备份保留策略）和 4712ca0（tunnel rotation），已复核其提交态差异，不涉及本文八类 Provider 的协议锚点。本文只分析已提交状态；有本地修改的参考仓库只读取 HEAD 提交态。

## 0. 执行结论

2026-09-11 版计划已经由 9c128d2、e3a9ed2、da2ae48 和 ae7fc88 大体实施完毕，但旧文档仍把 reasoning replay、统一恢复边界、Kiro Prompt Cache、SQLite authority、迁移、备份和回滚写成未来工作。继续照旧计划执行会重复建设并破坏已经验证的边界，因此本文整体替换旧计划，只保留已完成基线，并为本次参考仓库增量建立新的 N 系列编号。

分析起点最值得优先补强的不是增加 Provider，而是五组窄而深的问题；它们已在 0.1 所列提交中完成本地实现或保守门禁：

1. Antigravity 的 Gemini grounding/citation 当时尚未形成三 Surface、流式与非流式一致的完整映射；搜索模型选择、billing metadata 和少数会话边缘形状也缺少请求级证据。
2. Claude 429 分类当时会把 overage-only、模型级或含糊的限流信号扩大为 Account cooldown，影响固定账号的可用性。
3. Codex Responses 当时把语义为空的启动 announcement 当成业务提交，可能提前关闭 pre-commit 恢复；内部 metadata 也没有按精确层级清理。
4. Kiro 当时对裸 namespaced tool 名、profileArn fallback 和 Responses Compact 缺少保守合同。
5. CodeBuddy 当时对中断工具轮次、reasoning-only assistant 和新 CN 原生模型的处理落后于参考实现。

Cursor 与 Qoder 本轮没有发现新的可静态确认生产缺口；Grok 的新增质量重试属于启发式策略，误伤风险高，不直接采纳。它们的工作重点是保持证据新鲜度和关闭真实账号验收，而不是重写已有 executor。

### 0.1 实施结果快照

本轮已完成全部 P0 的本地实现/门禁与可离线判定的 P1/P3 差分；没有真实凭据的项目未被提升为 live verified。落地提交如下：

| 提交 | 结果 |
| --- | --- |
| `407a5e3` | Antigravity grounding/citation、模型搜索 capability snapshot、精确 metadata 与 conversation edge |
| `38af767` | Claude 429 作用域、CAQS opaque replay、初始 turn fingerprint、tool/terminal 边界 |
| `cc4e263` | Codex bootstrap/metadata、WebSocket 公平性与 request-lifecycle memory budget |
| `d80662b` | Kiro namespaced tool、profileArn fallback 与 Compact 零发网门禁 |
| `466291c` | Grok 只读质量观察；不重试、不轮换、不改写响应 |
| `56447b1` | Cursor/Qoder 提交态证据刷新；均无新生产 wire |
| `db65188` | CodeBuddy 11148 历史修复、reasoning/namespace 与取消生命周期 |
| `248e7c2` | 收口 Clippy、Phase-0 源码证据哈希与跨仓审计语法兼容；无 wire 行为变化 |
| `e5bfc34` | Antigravity Provider lifecycle 拆分与两条 OAuth rail 的私有 receipt 验收链；无真实输入时保持 `live_pending` |
| `6b0a0fe` | Claude Provider lifecycle 拆分与四个 operation 的私有 receipt 验收链；无真实输入时保持 `live_pending` |
| `069f3ef` | Codex Provider lifecycle 拆分、三个 GPT Image 2.5 variant 与 WS prewarm 的四个私有 receipt gate；付费探针降级为 `probe_only` |
| `8f72bb9` | Cursor Provider lifecycle facade 与 OAuth/API-key 双 rail schema-v2 私有 receipt gate；无真实输入时保持 `live_pending` |
| `269850a` | Grok Provider lifecycle 拆分与 inference/media/remote_compaction 三个 operation 的私有 receipt gate；无真实输入时保持 `live_pending` |
| `1124082` | Kiro Provider lifecycle facade 与四种 auth kind × 两个 region 的八路私有 receipt gate；无真实输入时保持 `live_pending` |
| `2cc8a01` | Qoder Provider lifecycle facade 与 Global OAuth/Global PAT/CN OAuth 三条 schema-v2 私有 receipt gate；未观察项继续阻止 live 提升 |
| `d81056c` | CodeBuddy Provider lifecycle facade 与 Intl/CN 两站 schema-v2 私有 receipt gate；fixture 不提升真实状态 |
| `4ad2293` | Antigravity CORE-N2：两条 Provider rail 的 sticky request-memory budget 覆盖 body、reasoning replay 与 grounding/citation retained state |
| `87fc0b0` | Claude CORE-N2：精确 Claude OAuth sticky budget 覆盖 body、压缩 SSE 和跨 chunk retained state |
| `3e0f5ff` | Cursor CORE-N2：OAuth/API-key 双 rail、三 Surface 的 sticky budget 覆盖 Agent plan、protobuf/gzip、h2、响应聚合、重试与 parked session |

| 类别 | 本轮已关闭 | 仍保持门禁/后续计划 |
| --- | --- | --- |
| Antigravity | AG-N1～N5、CORE-N2 `fixture_verified`；CORE-N1 Provider 切片与 reference-delta v2 已完成 | AG-N6 与两条 OAuth rail `live_pending`；compaction 继续关闭 |
| Claude | CL-N1～N4 与 CORE-N2 `fixture_verified`；CORE-N1 Provider 切片与 reference-delta v2 已完成 | OAuth inference、Max 5x/20x plan、Fable 5.1 四个 operation 独立 `live_pending` |
| Codex | CX-N1～N4 `fixture_verified`；CORE-N1 Provider 切片、CORE-N2 首个 request-memory 切片与 reference-delta v2 已完成 | GPT Image 2.5 三个 exact-model operation 与 WS prewarm 独立 `live_pending` |
| Cursor | CUR-N2 `reviewed_no_wire_delta`、CORE-N2 `fixture_verified`；CORE-N1 Provider facade 与 reference-delta v2 已完成 | CUR-N1 OAuth/API-key 两 rail 独立 `live_pending` |
| Grok | GR-N1 `fixture_verified`（观察-only）；CORE-N1 Provider lifecycle 与 reference-delta v2 已完成 | GR-N2 inference/media/remote_compaction 三个 operation 独立 `live_pending` |
| Kiro | KI-N1/N2 `fixture_verified`；KI-N3 fail-closed fixture；CORE-N1 Provider facade 与 reference-delta v2 已完成 | KI-N3 启用与 auth kind × region receipt `live_pending`；KI-05 shared cache 继续关闭 |
| Qoder | QD-N2 `reviewed_no_wire_delta`；CORE-N1 Provider facade 与 reference-delta v2 已完成 | QD-N1 三 rail 独立 `live_pending` |
| CodeBuddy | CB-N1/N2 `fixture_verified`；CB-N3/N5 共享实现已覆盖；CORE-N1 Provider facade、Intl/CN schema-v2 私有 receipt gate 与 reference-delta v2 已完成 | CB-N4 与两站真实 receipt `live_pending`，v4.1 runtime disabled |

CORE-N2 已完成 Codex、Antigravity、Claude 与 Cursor 四个 Provider 切片；Grok、Kiro、Qoder、CodeBuddy 仍待按各自 wire/owner 生命周期逐项推广。CORE-N1 与 EVID-N1 均已完成八个 Provider 切片。所有缺真实凭据的 LIVE-N1 operation/rail/site 继续保持门禁。

### 0.2 原始优先级摘要

| 优先级 | 必做项 | 目标 |
| --- | --- | --- |
| P0 | AG-N1、AG-N3、CL-N1、CX-N1、CX-N2、KI-N1、KI-N3 的 fail-closed 部分、CB-N1、CB-N2 | 修复可证明的数据丢失、错误作用域、过早提交或错误路由 |
| P1 | AG-N2、AG-N4～N6、CL-N2～N4、CX-N3、KI-N2、KI-N3 的启用阶段、CB-N3～N5、各 rail live receipt | 先差分或真实验收，再修改/开放能力 |
| P2 | CORE-N1、CORE-N2、EVID-N1 | 降低巨型热路径风险，统一生命周期内存预算和增量证据 |
| P3 | GR-N1 及低收益优化 | 只做诊断，不引入不可靠自动重试 |

## 1. 范围、方法和状态词

### 1.1 推荐参考仓库冻结点

| 类别 | 各目录 AGENTS.md 推荐参考 | 本次读取基线 | 工作树处理 |
| --- | --- | --- | --- |
| Antigravity | CLIProxyAPI、Antigravity-Manager | b773607e3e775、08402030c81d1 | 均干净 |
| Claude | CLIProxyAPI | b773607e3e775 | 干净 |
| Codex | CLIProxyAPI、codex2api | b773607e3e775、de41a5e3dfe9f | 均干净 |
| Cursor | OmniRoute | 02c663cdd0e85 | 22 项本地修改；仅用 HEAD |
| Grok | grok2api、sub2api | 906b9493b099d、ab99d56e9626e | grok2api 干净；sub2api 6 项本地修改，仅用 HEAD |
| Kiro | kiro.rs | be0c04219d9d1 | 干净 |
| Qoder | TokenRouter | 7faf9469bc695 | 干净；本地领先 origin/main，固定当前 HEAD |
| CodeBuddy | cli2api | 624874a0331f5 | 2 个未跟踪文件；仅用 HEAD |

外部仓库只是一轮明确协议研究的只读证据。它们不得成为 Cargo/npm 依赖、测试输入目录、CI checkout、运行时同步源或发布前置条件。需要吸收的最小 wire 事实必须冻结为本仓库自包含 fixture，并记录来源 commit、路径、提取时间和本仓库判定。

### 1.2 分析方法

本轮按以下顺序逐类核对：

1. 读取 Antigravity、Claude、Codex、Cursor、Grok、Kiro、Qoder、CodeBuddy 目录内 AGENTS.md，确认推荐项目和产品边界。
2. 固定参考 HEAD；参考工作树不干净时只用 git show、git log 和提交对象，不读取未提交内容作为证据。
3. 从旧分析冻结点向当前参考 HEAD 检查增量提交，再回到目标基线定位生产路径、合同、fixture 和测试。
4. 同时满足“参考行为明确、目标静态缺少等价处理、符合本产品不变量”才标记 confirmed_gap。
5. 目标可能已有等价通用处理、参考行为存在争议或只能靠真实服务判定时，先标记 differential_first 或 live_only，不以对齐为理由直接改生产代码。

### 1.3 状态词

| 状态 | 含义 | 实施规则 |
| --- | --- | --- |
| confirmed_gap | 已从目标生产路径确认不存在或作用域错误 | 先冻结失败 fixture，再实现最小修复 |
| differential_first | 静态证据不足，或目标已有相邻通用能力 | 先跑目标/冻结参考差分；只有红灯才改生产代码 |
| live_only | 离线无法证明厂商接受性、entitlement 或真实 wire | 缺真实凭据时保持 live_pending，不得用 mock 升级 |
| not_adopted | 与固定绑定、Server 产品边界或证据标准冲突 | 不进入实现 backlog，保留拒绝理由防止回流 |
| fixture_verified | 本仓库自包含 fixture 已验证实现或保守门禁 | 只代表离线合同，不得外推真实厂商接受性 |
| fixture_verified_existing_shared_* | 专项差分证明共享 bridge/guard 已等价覆盖 | 补专项测试与证据，不复制 Provider 私有分支 |
| reviewed_no_wire_delta | 推荐参考的提交态复核没有协议增量 | 只刷新 evidence，不修改生产 executor |
| live_pending / runtime disabled | 缺独立真实 receipt，或能力在运行时关闭 | 保持关闭/待验收；一条 rail 成功不提升其他 rail |

优先级与状态相互独立。P0 表示一旦差距成立会造成高风险，不表示可以跳过证据门禁；live_only 即使排在 P1，也不能在没有真实 receipt 时启用。

### 1.4 不可破坏的不变量

- Share、Provider Surface 和明确 Account 固定绑定。恢复只能发生在同 Provider、同 Account、同 rail、同 auth identity generation 内。
- 禁止账号池选择、跨账号轮换、跨 Provider fallback、跨站点 fallback，以及以“可用性”为理由静默改变身份。
- 首个下游业务输出提交后禁止透明 replay；所有恢复受统一 attempt 次数、分类预算和总耗时预算约束。
- 新状态写入继续通过 ServerStateInner 域方法，遵守 config → providers → accounts → usage → shares → ui_settings → sessions → oauth_logins 锁顺序。
- 不迁入桌面/Tauri、商业计费、Key 分销、签到、运营自动化、账号调度和外部项目 UI。
- token、Cookie、opaque reasoning、tool 参数、prompt、原始错误体和真实用户标识均按秘密处理，不进入日志、指标或 receipt。
- docs/provider/coverage.md 是生成文件，任何能力变化都修改合同源和生成器，不手工编辑该文件。

## 2. 已完成基线：禁止重复实施

### 2.1 落地提交

| 提交 | 已完成内容 |
| --- | --- |
| 9c128d2 feat(proxy): strengthen provider recovery and storage | 八类 reference delta 与 audit、统一 execution 原语、Antigravity/Grok replay、Kiro Prompt Cache、Provider fixture、SQLite shadow/authority、迁移/备份/回滚主体 |
| e3a9ed2 feat: safely recover provider account rate limits | Account 限流恢复控制面、当前代际校验和安全恢复入口 |
| da2ae48 chore: refresh provider contract evidence | compatibility window 与 writer inventory 证据刷新 |
| ae7fc88 fix(share): normalize permanent owner expiry | Share 永久 owner 到期语义收口 |

旧编号用于描述这批历史结果，现已冻结；本文新工作只使用 AG-N、CL-N、CX-N、CUR-N、GR-N、KI-N、QD-N、CB-N、CORE-N、EVID-N 和 LIVE-N，避免把旧项目重新实现一遍。

### 2.2 八类历史状态

| 类别 | 已离线验证 | 尚需真实证据但不应重写实现 |
| --- | --- | --- |
| Antigravity | AG-01～AG-03 为 fixture_verified | AG-04 compaction 为 live_pending，runtimeEnabled=false；两种 OAuth rail 分开验收 |
| Claude | 旧 CL-01～CL-06 本地合同为 fixture_verified | 厂商真实接受性仍按 operation/rail 保持 live_pending |
| Codex | CX-01、CX-02、CX-04 为 fixture_verified | CX-03、CX-05、CX-06 为 live_pending |
| Cursor | CUR-01、CUR-03 为 fixture_verified | CUR-02 的 OAuth/API-key rail 需各自 receipt |
| Grok | GR-01～GR-03 为 fixture_verified | GR-04、GR-05 为 live_pending，remote compaction 不开放 |
| Kiro | KI-01～KI-03 为 fixture_verified | KI-04、KI-05 为 live_pending，多副本 cache 不开放 |
| Qoder | QD-01、QD-03 为 fixture_verified | QD-02 的 Global OAuth、Global PAT、CN OAuth 三 rail 分开 pending |
| CodeBuddy | CB-01～CB-03 为 fixture_verified | CB-04 的 Intl/CN 双站保持 live_pending |

fixture_verified 只说明本仓库自包含合同已通过，不等价于真实订阅已验收。上述 live_pending 项由 LIVE-N1 统一追踪，不通过复制旧实现来“完成”。

### 2.3 横切安全与存储现状

以下能力已经存在，后续计划只允许补测试、修窄边界或渐进拆分：

- src/proxy/execution/context.rs、recovery.rs、terminal.rs、transport.rs 已承载固定绑定、AttemptBudget、CommitGuard、恢复和 transport 原语。
- 生产中的跨 Provider failover 和 excluded provider 选择入口已经删除，并有静态门禁。
- Antigravity/Grok replay 已具备 Provider 专属 scope、TTL/容量、snapshot CAS、代际漂移和敏感值保护。
- Kiro Prompt Cache 已从请求路径同步整份 JSON 写入迁为 bounded async writer、dirty generation、退避、shutdown flush 和 SQLite CAS。
- Provider、Account、Share、Usage 的 SQLite schema、引用图事务、generation/revision CAS、usage UPSERT 和 shadow_verified → prepared → committed authority 状态机已完成。
- config migrate-server-store、显式 apply、rollback export、committed backup 校验、WAL/损坏/磁盘满故障路径已完成。
- conformance、reference delta、registry/coverage/UI 一致性和 receipt 敏感字段已有 audit。

因此，本计划不再安排“引入统一恢复状态机”“把核心存储迁到 SQLite”“建立备份回滚”或“新增 replay 基础件”。这些描述在旧文档中已经过期。

## 3. 目标基线的结构风险

以下为最终集成基线 HEAD 的提交态行数：

| 文件 | 行数 | 当前风险 |
| --- | ---: | --- |
| src/proxy/forwarder.rs | 47,334 | 八类 Provider 编排、HTTP/SSE/WS、错误作用域和恢复分支仍高度集中 |
| src/state.rs | 34,495 | 域门面、后台任务和持久化编排过大；并非 SQLite 未完成，而是实现边界仍难审查 |
| src/proxy/transforms.rs | 11,168 | 多 Surface、多 Provider 转换共享文件，边缘语义容易横向回归 |
| src/proxy/stream_transforms.rs | 9,880 | 多种流事件和终态交织，提交时机改动影响面大 |
| src/repository/server_sqlite.rs | 3,107 | authority 已完成，但 schema、校验和迁移逻辑应继续按关注点拆分 |

结构优化必须与协议行为修复分离。先用 fixture 锁定 wire，再移动代码；不以文件行数下降作为单独成功指标。

## 4. Antigravity

### 4.1 已有能力

目标在分析起点已实现 managed OAuth、项目/tier/quota、模型目录、Claude/Gemini/OpenAI 三 Surface 桥接、function tools 与 web search 共存、reasoning replay、session scope/rollover、schema/transport/Retry-After 合同和同账号 pre-commit 恢复。起点的 src/proxy/adapters.rs 会对搜索请求无条件切换固定 Gemini fallback；AG-N2 已把它收敛为请求级能力快照与受证 fallback。普通请求的 requestType=agent 因 AG-N6 缺真实证据而保持不变。

### 4.2 参考增量

| 参考提交 | 新信号 |
| --- | --- |
| CLIProxyAPI ef63d2e7、7fcbdf88 | Gemini web search、groundingMetadata、citation、Unicode offset 和流事件顺序 |
| CLIProxyAPI a9e92b81 | 请求级 model metadata/capability 保留 |
| CLIProxyAPI b681a1e0 | 中途 system/developer 保持时序并降级为 system-reminder |
| CLIProxyAPI 8c984672 | 孤立 function output 转为普通 user text，避免静默丢数据 |
| CLIProxyAPI fd3e6623 | 空 text part 不关闭仍活动的 Claude content block |
| Antigravity-Manager 734e2bde | requestType 根据 tools/tool history 动态设置 |
| Antigravity-Manager 9fd77989 | 发往 Gemini 前过滤 Claude billing metadata |

### 4.3 新增项

| ID | 状态 | 优先级 | 差距与目标 |
| --- | --- | --- | --- |
| AG-N1 | fixture_verified | P0 | 已建立 Gemini grounding 到 Responses、Chat、Claude 的 citation 映射，覆盖流/非流、Unicode、重复 chunk 与事件顺序 |
| AG-N2 | fixture_verified | P1 | 已按请求冻结 per-model capability 与 Account generation；未知能力使用受证 fallback，漂移发网前冲突 |
| AG-N3 | fixture_verified | P0 | 仅过滤 Gemini 目标的独立单行 billing metadata；多行正文、消息与参数保持 |
| AG-N4 | fixture_verified | P1 | 中途 system/developer 保序包装；孤立/错配 output 可逆降级，不伪造 pairing |
| AG-N5 | fixture_verified | P1 | 空 text part 与活动 block 生命周期已由 fixture 冻结 |
| AG-N6 | live_pending_no_change | P1 | 两个参考结论冲突且无真实 receipt；保持现有 requestType 行为 |

AG-N1 的验收矩阵必须至少包含：

- groundingChunks 中 web URI/title、支持索引缺失、重复来源、无效索引和多候选；
- ASCII、中文、emoji、组合字符的 offset；明确目标 Surface 使用 byte、Unicode scalar 还是 UTF-16 单位，不能靠 Rust 字节下标猜测；
- Responses annotation、Chat citation/annotation 和 Claude web search result 的语义等价，而非强行产出同一 JSON；
- 搜索 item、文本 delta、citation 和 terminal 的稳定顺序；citation 不得晚于 terminal；
- URL/title 只出现在下游协议，不写原文日志或 metrics label。

AG-N2 不允许继续以“看到 google_search 就固定改为 gemini-2.5-flash”作为长期策略。请求开始时解析 model capability，保存 capability revision、provider revision、runtime fingerprint 和账号身份代际；若请求期间 catalog 更新，只影响下一请求。能力未知时 fail closed 或保留明确的已验证 fallback，不能猜测新模型支持搜索。

AG-N3 采用最窄规则：只处理 system 区域中内容恰好为一个已知 metadata 行的独立文本块。多行 block、普通正文中的同名前缀、messages 内文本和 tool 参数必须原样保留。Anthropic → OpenAI instructions 与 Anthropic → Gemini 共用 matcher，但各自显式调用；禁止做全局字符串 replace。

AG-N4 的差分 fixture 至少包括首段 system、用户轮次后的 developer、连续多个 system、孤立 tool result、错配 ID 和正常成对工具轮次。只有目标当前输出确实丢时序/数据时才改转换；包装文本必须是稳定、可逆识别的本仓库格式，不能伪造 tool pairing。

AG-N6 必须分别验证纯文本、只带 tools、带历史 tool call/result、web search 和混合工具。CLIProxyAPI 与 Antigravity-Manager 结论冲突时，以本仓库真实账号 receipt 为准；没有 receipt 就保持当前行为和 live_pending。

### 4.4 不采纳

不采纳账号池 balance failover、跨项目选账号、模糊文本 429 分类、把上游错误改写成“友好回答”、固定常量派生 capsule 密钥，以及未经真实证据开放 compaction。

### 4.5 后续实施状态

`e5bfc34` 已把 reasoning/session replay lifecycle、结构化 limit/cooldown、Retry-After 和 credential-scoped transport orchestration 实际迁入 `src/proxy/providers/antigravity/`；`forwarder.rs` 只保留共享 attempt budget 与通用 dispatch，wire、terminal、usage、metrics key 和同绑定恢复边界不变。两条 rail 由 `scripts/smoke/antigravity-real.mjs` 分别核对 Account、Claude/Gemini Provider、Share、fresh catalog、generation scope 和当前 target commit；fixture 只能得到 `contract_verified/live_pending`，不能生成 `live_verified`。

`4ad2293` 完成 CORE-N2 的 Antigravity 切片：只对精确的 `antigravity_oauth` / `agy_oauth` binding 启用 sticky request budget，统一核算 raw/decoded/normalized body、reasoning replay cache/snapshot/stream accumulator 与 grounding/citation/web-search retained state。Share 和 pinned Provider test 均在发网前建立预算；重试共享同一预算，耗尽时稳定 503、取消上游且禁止 replay，reservation 跟随最终 `Bytes` owner 在最后一个 view drop 后释放。该切片本身不因 App 相同而启用 Claude OAuth 或 Gemini CLI；Claude OAuth 后续由自己的精确 CORE-N2 切片独立启用。

Antigravity reference delta 已保留原 schema v1 sources/capabilities，并追加 17 条不可变 v2 observation；`AG-OBS-0017` 将外部 replay 局部边界明确标为 differential，而非统一生命周期预算的直接证明。audit 会离线验证 observation ID、source digest、target contract/fixture 和 implementation commit 格式；`--check-sources` 仅可选复核冻结 Git object，不进入构建或运行时。当前没有真实凭据，`antigravity_oauth`、`agy_oauth`、AG-N6 和 compaction 继续保持 `live_pending`/runtime disabled。

## 5. Claude

### 5.1 已有能力

目标已有 Claude OAuth wire、direct beta passthrough、cache TTL、trailing usage、structured output、interleaved tools、helper request id、语义终态、取消传播和额度窗口持久化。通用 terminal guard 已能避免许多 post-commit replay；CL-N 系列重点不是重做 Claude executor，而是收窄 429 作用域。随后完成的 CORE-N2 切片已把精确 Claude OAuth 的 body、压缩 SSE 与 retained stream state 纳入统一 sticky budget。

### 5.2 参考增量

| 参考提交 | 新信号 |
| --- | --- |
| CLIProxyAPI 44eaef00 | overage-only 与 model-scope 限流，不应一律冷却整个账号 |
| CLIProxyAPI 7c32971b | 成功终态之后的下游断连不应反记为上游 stream failure |
| CLIProxyAPI 2bcebaa8 | tool pairing 修复和 standalone output |
| CLIProxyAPI 377c315f | billing fingerprint 锚定初始 turn，避免多轮漂移 |
| CLIProxyAPI 75ce6352 | CAQS v4 reasoning signature |
| CLIProxyAPI fc96a87f | organization-hashed credential identity |
| CLIProxyAPI 2e6b1d83 | thinking replay 的每 session/turn/block、进程总量、TTL 与 CAS 边界；只作为 retained state 应有界的差分信号 |

### 5.3 新增项

| ID | 状态 | 优先级 | 差距与目标 |
| --- | --- | --- | --- |
| CL-N1 | fixture_verified | P0 | 仅共享 5h/7d 窗口明确耗尽且 reset 合法时写 Account cooldown；其余保持 model/Fable/request scope |
| CL-N2 | fixture_verified | P1 | CAQS v4 作为 opaque signature 原样往返，不解析、不生成、不记录 |
| CL-N3 | fixture_verified | P1 | 初始 turn fingerprint 在 system migration、cache 与 retry rewrite 前冻结；未迁入 cloaking |
| CL-N4 | fixture_verified | P1 | standalone/错配 tool output 可逆降级；合法 message_stop 后立即完成并释放上游 body |

实施前，claude_fable_only_rejected 只把 5h/7d 明确 allowed 或 allowed_warning 视为共享窗口健康；classify_claude_rate_limit 会把 unified rejected、7d_oi rejected 的含糊组合提升到 Account。CL-N1 已按如下决策表收窄并冻结：

| 证据形状 | 允许的作用域 |
| --- | --- |
| 5h 或 7d 明确 rejected，且 reset 合法 | Account shared-window cooldown |
| overage/7d_oi rejected，5h/7d 明确健康 | Fable/overage 或精确 model scope |
| status 缺失但 utilization 明确小于 1.0 | 不得据此冷却 Account |
| unified rejected，但子窗口显示共享额度健康 | 不得覆盖更具体证据 |
| org_spend_cap_reached、disabled reason、representative claim | entitlement/organization 诊断或精确模型拒绝，不得默认 Account cooldown |
| header 缺失、冲突或无法解析 | 最窄的 exact-model/unknown-429 cooldown，不能扩大 |

reset、Retry-After 和 utilization 仍需保留上限与时间解析保护。解析器输出应包含 scope、reason、evidence completeness 和 until；日志只记低基数 reason，不记 header 原值。Account 状态只在当前 auth identity generation 仍匹配时写入。

CL-N2 只验证签名 wire 和跨 Surface 保真，不自造签名，不记录 opaque signature。CL-N3 不采纳参考项目的浏览器伪装或 credential cloaking；只研究初始 turn 作为 cache/fingerprint 输入是否能避免同一会话漂移。CL-N4 的“终态后断连”以已经观测到成功 terminal 为界，不得掩盖 terminal 前断连。

### 5.4 不采纳

不迁入 organization-hash 账号迁移、credential cloaking、浏览器指纹模拟或参考项目的多账号管理。除非厂商 wire 和本仓库身份模型共同要求，否则 Account ID 继续由本仓库明确绑定控制。

### 5.5 后续实施状态

`6b0a0fe` 已把 Claude quota header 观测、429 scope 应用、Fable/Account/model cooldown 和 transport replay-safe 决策迁入 `src/proxy/providers/claude/`；`forwarder.rs` 保留共享 attempt budget、dispatch、terminal 与 usage 编排。该切片只移动 Provider lifecycle 边界，不改变 wire、错误分类、固定 Provider/Account 绑定或 pre-commit 恢复预算。

`scripts/smoke/claude-real-receipt.mjs` 为 `oauth_inference`、`max_5x_plan`、`max_20x_plan`、`fable_5_1` 建立四个相互独立的私有 receipt gate，分别固定当前 target commit、Account generation、Provider binding、Share revision、模型与 plan evidence。fixture 最多产生 `contract_verified/live_pending`；真实 receipt 必须写在仓库外、权限为 `0600`，任一 operation 成功不能提升其他 operation。当前没有真实凭据，四项 receipt 均为 `null`，不得标记 `live_verified`。

`87fc0b0` 完成 CORE-N2 的 Claude 切片：只为精确的 `claude_oauth` Provider 启用 sticky request budget，普通 `claude`、`claude_auth` 与其他 Claude App Provider 不受影响。Share 与 pinned Provider test 在发网前核算 raw body；压缩请求继续合计 raw/decoded/normalized body。压缩 SSE 在聚合 wire body 时动态 reserve，解压上限取配置值与预算剩余值的较小者，decoded `Bytes` 由 owner reservation 保持到最后一个 view drop。流式路径在增长前预留并在处理后收缩，覆盖 Anthropic pending/open block/tool partial JSON、terminal detector、usage/error decoder、工具名 alias/buffer 与 stream transformer；耗尽后不进行 transport/body replay，HTTP 未提交时返回稳定 503，已 200 时发送同一错误码的终止帧。

Claude reference delta 已提升为 append-only schema v2：原 schema v1 `sources`、`localContracts`、`realAcceptance` 由 digest 固定，两个只读 source snapshot 保持不变，当前共有 13 个 source delta 和 15 条不可变 observation。`CL-OBS-0012` 明确拒绝 organization-hash credential identity；`CL-OBS-0015` 将参考 replay 的局部容量边界标为 differential，而不是统一 Claude OAuth 生命周期预算的直接证明。audit 始终从本仓库已提交 Git object 校验 target baseline/implementation tree 与 anchors，只有显式 `--check-sources` 才读取外部已提交对象。四个真实 operation 仍全部 `live_pending`。

## 6. Codex

### 6.1 已有能力

目标已有 Responses/Responses Lite、HTTP/SSE/WS、多轮 Turn-State 转发、绝对 first-event deadline、body/event 上限、WS pool 独占、WS→HTTP pre-commit fallback、语义终态和 unified attempt budget。参考的新能力不能被误写成“目标完全没有”。

### 6.2 参考增量

| 参考提交 | 新信号 |
| --- | --- |
| CLIProxyAPI e696ea47、f702bc1a | Turn-State 与 Responses Lite 保真 |
| CLIProxyAPI cb73cd99、6e307553 | 启动期 keepalive/空 announcement 缓冲与绝对时间上限 |
| CLIProxyAPI b5ba02c2 | 大 payload 写入期间避免 pong starvation |
| CLIProxyAPI 4311ae87 | search capability 必须是 per-model 显式能力 |
| codex2api dc47d131 | 只删除 input item 顶层 internal_chat_message_metadata_passthrough |
| codex2api 19ee8db4 | request-lifecycle memory budget |

### 6.3 新增项

| ID | 状态 | 优先级 | 差距与目标 |
| --- | --- | --- | --- |
| CX-N1 | fixture_verified | P0 | 空 startup announcement 仅在闭集且语义为空时为 Lifecycle；HTTP JSON/SSE/WS/Lite 共用分类器 |
| CX-N2 | fixture_verified | P0 | 只删 `input[*]` 顶层内部 metadata；content、arguments、嵌套同名字段与顺序保持 |
| CX-N3 | fixture_verified | P1 | 已拆单 reader/writer actor、有界队列与 32 KiB continuation；双向 ping/pong 和取消不被大上传饿死 |
| CX-N4 | fixture_verified | P2 | request-scoped sticky memory budget 已覆盖 HTTP/WS 全生命周期，耗尽稳定 503/error+close 且禁止 replay |

实施前，src/proxy/response_semantics.rs 的 classify_value 只把 response.created、response.in_progress 和 response.queued 判为 Lifecycle，其余非终态默认 Business，因此空 response.output_item.added、response.content_part.added 和 response.reasoning_summary_part.added 会过早提交。CX-N1 已用下述闭集语义替换该行为。

CX-N1 必须采用闭集且检查载荷是否语义为空：

- 已知 added announcement 只有在 item/part 不含文本、工具参数、server operation、错误或其他下游可见数据时才是 Lifecycle；
- 空 role/index/id/status 等启动外壳可缓存或透传，但不能触发 CommitGuard；
- 未知 item type、未知 part type、web/file/computer 等 server operation 一律视为 Business，不能为了恢复而延迟真实输出；
- 绝对 first-event deadline 继续从首次等待开始计算，keepalive/空 announcement 不重置；
- overload/error 若出现在首个真实业务输出前，仍可在同账号预算内恢复；之后必须原样终止。

CX-N2 的 mutation tests 应同时证明：

- input[0].internal_chat_message_metadata_passthrough 被删除；
- input[0].content 内同名字段保留；
- function arguments 字符串或对象内同名字段保留；
- input 之外的用户扩展字段不被递归清理；
- 重复执行幂等，且不改变 item 顺序、类型和 hash 之外的字段。

CX-N3 先测单个大 outbound frame、持续上传 tool arguments、服务端 ping、客户端 ping、取消和背压。只有看到 pong 超时或 control frame 延迟超过冻结阈值时，才实施单 writer ownership 与高优先 control queue；不得因参考项目有修复就重写当前 WS pool。

CX-N4 统一核算压缩前/解压后 body、transport pending、semantic bootstrap buffer、tool argument 累积、normalized event 和待发送队列。预算耗尽要产生稳定的 capacity/protocol 错误，释放内存并取消上游；不得 fallback 到另一个账号。预算值进入配置合同和审计，但 secrets 与内容不能进入指标。

### 6.4 不采纳

不采纳账号级静态 Turn-State 注入、跨请求共享未知状态、background preemption lane、跨账号 capacity routing，以及为了缓存而延迟未知业务事件。

CX-R1 明确拒绝从参考图片实现迁入商业计价、自动 driver-model fallback、账号池选号、轮换或跨账号恢复。`069f3ef` 把 Codex 429 scope、reset 解析和 capacity retry 决策收敛到 `src/proxy/providers/codex/`，保持原 wire、固定 Provider/Account 和共享总 attempt budget。私有 receipt harness 将 `gpt_image_2_5`、`gpt_image_2_5_flare`、`gpt_image_2_5_sunburst`、`ws_prewarm` 固定为四个独立 operation；前三项同时要求 generation/edit/Responses、usage/error/cooldown、capability、多副本、Cloudflare 与零 fallback/replay，WS 项要求 upstream acceptance、至少五个样本的 TTFB 收益、连接复用和严格 pre-commit WS→HTTP。当前无真实凭据，四份 receipt 均为 `null`，状态保持 `live_pending`。

## 7. Cursor

### 7.1 对比结论

OmniRoute 从旧冻结点到 02c663cdd0e85 没有新的 Cursor wire、protobuf、auth 或 session 专项提交。目标已经具备 OAuth/API-key 双 rail、protobuf/open-sse、session/continuation、模型目录和自包含 fixtures，因此 CUR-N2 没有发现需要新增生产 wire 的 confirmed_gap；后续实施的 CORE-N2 是独立横切容量治理，不改变该结论。

OmniRoute 工作树有 22 项本地修改，全部排除在证据之外。后续只能把新的已提交 Cursor 变更纳入 reference delta；不能引用工作树文件、截图或本地运行结果。

### 7.2 新增项

| ID | 状态 | 优先级 | 工作与退出条件 |
| --- | --- | --- | --- |
| CUR-N1 | live_pending | P1 | OAuth 与 API-key rail 仍需分别产生本仓库 receipt；一条成功不得外推另一条 |
| CUR-N2 | reviewed_no_wire_delta | P2 | 已复核 OmniRoute@02c663c；六个 wire 对象无变化，22 项工作树修改排除，生产代码不变 |

CUR-N1 继续保持 Account 固定绑定。credential kind、endpoint、session identity 和 model catalog 必须写入脱敏 receipt 的结构字段；token、machine identity 原值和 prompt 不得落盘。

`8f72bb9` 已以 `src/proxy/providers/cursor/` 建立共享 forwarder 与既有 Cursor 协议实现之间的生命周期 facade，收敛 adapter、模型选择、native dispatch 和 h2 timeout mapping；binding、lease、Share、attempt、terminal、usage、protobuf 与 session 所有权不变。OAuth/API-key receipt 升为 schema v2/harness revision 2，分别绑定当前 target commit、Provider/runtime、Share、credential generation、精确 `*-fast` model、22 项检查、10 份 body hash、5 项测量、固定恢复决策与 decoy/secret scan；仓库外真实文件要求 `0600`。没有真实输入时两条 rail 仍为 `live_pending`。

`3e0f5ff` 完成 CORE-N2 的 Cursor 切片：只对精确 `cursor_oauth` / `cursor_apikey` binding 启用 sticky budget，并同时覆盖 Claude、Codex、Gemini Surface。最终 normalized body 在发网前计入；Agent plan/JSON、工具 schema、图片、protobuf、h2 write queue/parser、gzip expansion、decoded/pending frame、错误 body、SSE/JSON 聚合、semantic retry 与 completed-response replay clone 均共享同一请求预算。OAuth 401 和 semantic recovery 在耗尽后返回 `DeniedBudget`，不发起 replay。

parked session 会保留 request reservation；continuation 先向新请求预算预留 session、parser 与 pending frame，再释放旧预算，close、expiry 与失败 guard 均清理 retained state。四种已提交 SSE Surface 使用稳定 `cc_switch_request_memory_exhausted` 终止码，usage 归类 `memory_capacity`，Provider outcome 归类 capacity shed。专项验证为 Cursor 304/304、request-memory 16/16、Clippy `-D warnings` 通过。

Cursor reference delta 已迁为 append-only schema v2：原 schema v1 的十个历史字段逐项由 digest 固定；一个明确排除 22 项工作树改动的 OmniRoute committed-object snapshot 保持不变，当前共有 8 条不可变 observation，完整映射 CUR-01～03、CUR-N1/N2、CORE-N1、CORE-N2、LIVE-N1。`CUR-OBS-0008` 只把参考的 16 MiB frame ceiling、rolling-buffer splice、5 分钟/100 session 与 close cleanup 作为“retained state 应有界”的差分信号；`evidenceExtensions` 记录 `CORE-N2-CURSOR=fixture_verified`。每条记录绑定 source commit/tree/path/symbol/digest 与本仓库 committed target baseline/implementation object；默认审计不读取外部仓库，只有显式 `--check-sources` 才复核冻结对象。

### 7.3 不采纳

不迁入 OmniRoute 的 Next.js/Electron/桌面数据库、运营 UI、多账号路由或未提交工作树能力。

## 8. Grok

### 8.1 对比结论

目标已有 reasoning replay、上游明确拒绝 opaque reasoning 后的同账号 pre-commit 恢复、root union adapter、Responses/Chat/Claude 三 Surface、usage 与 terminal 合同。grok2api@7f3f3d3c 新增低 visible token 和低 plaintext-ratio 时的质量重试，但该策略根据输出外观猜测失败，会把合法短答、纯工具调用、结构化输出和低文本推理误判为坏结果。

### 8.2 新增项

| ID | 状态 | 优先级 | 工作与退出条件 |
| --- | --- | --- | --- |
| GR-N1 | fixture_verified_observation_only | P3 | JSON/SSE/WS 已增加低基数质量形状指标；逐字节透传，不自动重试、轮换或改写 |
| GR-N2 | live_pending | P1 | 推理/媒体矩阵和 remote compaction 仍缺真实 receipt；无证据能力继续关闭 |

GR-N1 指标只能记录低基数 outcome kind、是否有 terminal/tool/visible text 和有界长度桶，不能记录 plaintext、reasoning 或比例对应的原文。如果未来厂商提供明确错误码或 receipt 证明某形状必然失败，也只能在同账号、pre-commit、总预算内 fail closed 或重试一次；不能换账号。

### 8.3 不采纳

明确不采纳 visible token 阈值、plaintext-ratio 阈值、内容关键词或“看起来不像回答”驱动的生产重试，也不采纳账号池和跨账号 balance failover。

`269850a` 已把 Grok replay scope 派生、snapshot ownership、CAS 清理/提交与 Provider/Account/Share generation fence 收敛到 `src/proxy/providers/grok/`；HTTP/WS wire、固定绑定、共享 attempt/10 秒预算、一次 pre-commit 明确拒绝恢复和零 post-commit replay 均未改变。LIVE-N1 同时新增 inference、media、remote_compaction 三个独立私有 receipt gate，绑定当前 target commit、Provider/runtime、Account auth/token generation、Share revision、签名用户、精确 model/session/turn、fresh catalog、checks/body hashes/measurements、恢复决策、decoy counters 与 secret scan；真实文件必须位于仓库外且为 `0600`。`grok-oauth-real.mjs` 仍是 `probe_only/live_pending`，当前三份 receipt 均为 `null`。

Grok reference delta 已迁为 append-only schema v2：`269850a` 中原 schema-v1 文件 SHA-256、target commit/tree、全部十个历史字段的 canonical/逐字段 digest 均被冻结；新增 grok2api 干净 snapshot、明确排除 6 项工作树修改的 sub2api snapshot，以及 10 条不可变 observation，完整映射 GR-01～05、GR-N1、CORE-N1、LIVE-N1、GR-R1。每条记录绑定 source commit/tree/path/symbol/digest 与本仓库 committed target object；GR-R1 将启发式自动重试、账号池/轮换、商业复合路由/计价和 soft quota gate 固定为 reject，默认审计不读取外部仓库。

## 9. Kiro

### 9.1 已有能力

目标已有 credential/profileArn region 推导、runtime/api region 规范化、模型目录 context window、Prompt Cache、usage 守恒、异步 SQLite 持久化、Claude Code builtin tool bridge 和同账号 401 恢复。kiro.rs@3219d1c 的 region 修复与目标现有实现等价；db3e912 的 context_window 也已由 catalog token limit → audited static → 200k default 的优先级覆盖，不重复实施。

### 9.2 参考增量

| 参考提交 | 新信号 |
| --- | --- |
| kiro.rs f413e7d | 上游返回裸 child name 时恢复 namespaced tool |
| kiro.rs 3194bb2 | 带 profileArn 的 usage 请求在特定拒绝后回退无 ARN |
| kiro.rs 0b8c7de、d62054f、13763b6 | Kiro-backed Responses Compact 的结构、缓存 usage 和测试修复 |

### 9.3 新增项

| ID | 状态 | 优先级 | 差距与目标 |
| --- | --- | --- | --- |
| KI-N1 | fixture_verified | P0 | 当前请求注册表按精确映射→完整名→唯一 child→普通名解析；歧义稳定失败关闭 |
| KI-N2 | fixture_verified | P1 | 仅带 ARN 的 403 或两个明确 400 形状允许同 host 去 ARN；其他错误不 fallback |
| KI-N3 | fixture_verified_fail_closed / live_pending | P0/P1 | Kiro/Amazon Q Compact 发网前稳定拒绝且 decoy 为零；启用仍需差分与真实 receipt |

实施前 map_tool_name 只为 builtin rename 和超长名称保存 tool_name_map；若声明 mcp__repo__search 而 Kiro 回裸 search，original_tool_name 精确查找会丢 namespace。KI-N1 已把解析顺序固定为：

1. 精确匹配实际发给上游的名称映射；
2. 精确匹配原始完整声明名；
3. 从本请求声明集合计算 child name；只有候选恰好一个时恢复完整名；
4. 零候选保留经过验证的普通名，多个候选返回协议错误，不猜 namespace。

候选索引是 request-scoped，只含当前请求的工具声明，不跨 Account、Share、session 或请求缓存。覆盖 mcp__a__read 与 mcp__b__read 歧义、builtin 映射、超长 hash、同名普通工具、流式分块参数和三 Surface 输出。

实施前 fetch_usage_limits 除 401 外几乎任何错误都会从带 ARN 回退到无 ARN，再尝试 CodeWhisperer host，可能把 429、5xx、body decode 和网络故障伪装成成功。KI-N2 已冻结并实现以下精确 status/body 边界：

- 401/403 的身份语义分别处理，401 不重放到其他 host；
- 只有参考证据明确允许的 403 或特定 400 才去掉 profileArn；
- 429 保留 Retry-After/限流作用域，不 fallback；
- 5xx、超时、TLS、decode 和结构不合法直接返回原始分类；
- 是否允许 q host → CodeWhisperer host 必须单独有 auth kind/region 证据，且仍使用同一个 Account credential。

实施前 forward_claude_kiro 会接收 CodexResponsesCompact 并误走普通 Kiro inference。KI-N3 的 P0 已完成，P1 仍保持真实门禁：

- P0：在发网前识别 Kiro/Amazon Q + CodexResponsesCompact，返回稳定 unsupported/fail-closed 错误；增加 decoy upstream，证明零请求。
- P1：冻结请求 capsule、cache usage、terminal、错误、stream/non-stream 和 context limit 的差分 fixture；再用绑定账号取得真实 receipt。全部通过后才加入独立 compact executor，不能复用普通生成响应“看起来能用”就开放。

### 9.4 不采纳

不因参考项目支持就直接启用 remote compaction，不迁入 Redis/多副本 cache、账号 affinity 或调度；不扩大 profileArn fallback 来提高表面成功率。

`1124082` 已把 Kiro/Amazon Q 产品边界、canonical request/model/session、Account-bound catalog/region/request preparation、本地 CountTokens、keepalive、stream error 与 subscription throttle 收敛到 `src/proxy/providers/kiro/`；AWS EventStream/wire 转换仍由 `src/proxy/kiro.rs` 持有，Share/Account lease、usage、terminal、共享 attempt budget 与同账号 401 replay 仍由 forwarder 持有。LIVE-N1 同时新增 Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 八个独立私有 receipt gate，分别绑定当前 target commit、Account auth/token generation、Claude/Codex Provider revision/runtime、Share revision、签名用户/session、exact model、两份 fresh catalog、checks/body hashes/measurements、恢复决策、decoy counters 与 secret scan；真实文件必须位于仓库外且为 `0600`。当前八份 receipt 均为 `null/live_pending`，receipt 不会自动开启 KI-N3 Compact 或 KI-05 shared cache。

Kiro reference delta 已迁为 append-only schema v2：`1124082` 中原 schema-v1 文件 SHA-256、target commit/tree、全部十个历史字段的 canonical/逐字段 digest 均被冻结；新增干净的 `kiro.rs@be0c042` / tree `5e656c1` committed-object snapshot，以及 13 条不可变 observation，完整映射 KI-01～05、KI-N1～N3、CORE-N1、LIVE-N1、KI-R1～R3。每条记录绑定 source commit/tree/path/symbol/digest 与本仓库 committed target object；三条 reject 明确排除跨 Account/auth-kind/region/Provider 或宽 profile/host fallback、Redis/session-affinity/cache 路由，以及无独立执行器与真实证据的 remote-compaction 自动启用。默认审计不读取外部仓库，只有显式 `--check-sources` 才复核冻结对象。

## 10. Qoder

### 10.1 对比结论

TokenRouter 从 3488b4a9 到 7faf9469 的 Qoder/COSY 路径净变化仅是 qoder_gateway_handler.go 对全局错误 helper 签名的适配，没有新增 Qoder wire、auth、signing、model、quota 或 session 行为。目标继续以官方 Qoder CLI 自包含 oracle 为第一来源、TokenRouter 已提交状态为第二来源。

目标已有 Global OAuth、Global PAT、CN OAuth 三 rail、Device/refresh、站点化 machine identity、COSY signing/session、目录/effort/context capability、三 Surface、tools/reasoning/usage、严格 terminal+EOF、quota 和同账号 pre-commit 401。本轮没有新的生产 confirmed_gap。

### 10.2 新增项

| ID | 状态 | 优先级 | 工作与退出条件 |
| --- | --- | --- | --- |
| QD-N1 | live_pending | P1 | Global OAuth、Global PAT、CN OAuth 仍需分别产生 receipt；三条 rail 状态互不继承 |
| QD-N2 | reviewed_no_wire_delta | P2 | TokenRouter@7faf946 仅共享 error helper 签名适配；官方 CLI oracle 仍为第一来源，生产代码不变 |

三条 rail 的成功状态互不继承。升级 oracle 时必须用 mutation 证明 endpoint、header、body、signature、machine identity 任一单边漂移会红灯；禁止 fixture 与 Rust 同时“顺手修改”造成假绿。

CORE-N1 已将 exact Account binding、Share/用户/session conversation scope、live catalog model/payload preparation、generation fence 与 Qoder throttle 收敛到 `src/proxy/providers/qoder/`；codec/runtime primitive 仍分别位于 `proxy::qoder` / `proxy::qoder_runtime`，Share/Account lease、usage、terminal、共享 attempt budget 与同账号 pre-commit 401 replay 决策继续由 forwarder 持有。LIVE-N1 的三条 rail receipt 已升级为 schema v2，分别绑定 target commit/harness、Account auth/token generation、三个 Surface Provider revision/runtime digest、Share revision、site/rail、exact model、三份 fresh catalog snapshot 和六个 body hash。当前 harness 未观察 login/rotation、权威空目录与受控两段 401，所以真实 happy-path 仍只可标记 `partial_live_verified/live_pending`，fixture 仍为 `contract_verified/live_pending`。

Qoder reference delta 已迁为 append-only schema v2：`2cc8a01` 中原 schema-v1 文件 SHA-256、target commit/tree、全部 11 个历史字段的 canonical/逐字段 digest 均被冻结；新增干净的 `TokenRouter@7faf946` / tree `b205061c` committed-object snapshot，以及 10 条不可变 observation，完整映射 QD-01～03、QD-N1/N2、CORE-N1、LIVE-N1、QD-R1～R3。每条记录绑定 source commit/tree/path/symbol/digest 与本仓库 committed target object；三条 reject 明确排除账号池/轮换/affinity 与跨 Account/rail/site/Provider fallback、商业计费/Key/渠道/维护调度/多租户后台，以及外仓运行时依赖、fixture/参考成功冒充 live 和缺唯一 terminal + EOF 仍成功。默认审计不读取外部仓库，只有显式 `--check-sources` 才复核冻结对象。

### 10.3 不采纳

不纳入 TokenRouter 的商业计费、Key 管理、渠道配置、账号维护调度和多租户管理后台。

## 11. CodeBuddy

### 11.1 已有能力

目标已实现 Intl/CN 固定站点、cookie-bound OAuth、flow jar/lease/TTL、site + uid + enterpriseId 身份闭合、rotation-safe refresh、约 24h session refresh、12153 needs-relogin、配置目录、三 Surface、tools/usage、严格 terminal+EOF、quota 白名单和同账号 pre-commit 401。旧 CB-01～CB-03 的空 message、空 delta、tools/tool_choice 归一已经 fixture_verified，本轮不重复实现。

### 11.2 参考增量

| 参考提交 | 新信号 |
| --- | --- |
| cli2api cdc80d6 | 中断工具轮次修复，关联业务码 11148 |
| cli2api a9ae393 | namespace tools、reasoning-only 与 Responses reasoning 合并 |
| cli2api 6ac6f83 | deepseek-v4.1-flash 顶层 reasoning 字段 |
| cli2api d361559、eef5b2c | 精确 native model ID，停止错误 alias rewrite |
| cli2api 76c4dab | stream cancellation 专项覆盖 |

### 11.3 新增项

| ID | 状态 | 优先级 | 差距与目标 |
| --- | --- | --- | --- |
| CB-N1 | fixture_verified | P0 | 完整轮次原样保留；中断/孤立/重复/错配/截断历史修复 11148，不伪造 result |
| CB-N2 | fixture_verified | P0 | reasoning 先规范化后过滤；Responses reasoning/message/function call 合并为同一 turn |
| CB-N3 | fixture_verified_existing_shared_bridge | P1 | Claude、Chat、Responses 声明/call/result 闭环通过；不采用裸 child fallback |
| CB-N4 | live_pending_runtime_disabled | P1 | 缺 CN 目录/receipt；`deepseek-v4.1-flash` 不进入 reviewed allowlist，不泛化专属字段 |
| CB-N5 | fixture_verified_existing_shared_guard | P1 | 截断、重复/后置 terminal、marker 前后取消通过；未增加 Provider 私有 guard |

实施前 build_codebuddy_payload 在把 reasoning_content 改名为 reasoning 之前先执行 messages.retain(codebuddy_message_is_semantic)，而 predicate 不识别 reasoning 字段，因此 content 为空但含 reasoning_content 的 assistant 会被删除。CB-N2 已改为先 canonicalize reasoning，再做语义判断；reasoning-only、tool-call-only 和 tool-result 视为有效，真正全空 message 仍按旧 CB-01 规则清理。

CB-N1 已按以下边界冻结“完整工具轮次”：

- 保留 ID 唯一、assistant call 与后续 result 完整配对的轮次；
- 对历史中断 call、缺 result、孤立 result、重复 ID、截断 arguments 和跨 turn 错配分别冻结 11148 fixture；
- repair 只能丢弃或降级不可发送的历史片段，不能编造成功 result、修改用户 tool output，或把当前正在生成的 partial call 当历史清理；
- repair 后重新验证 tool name、ID 和顺序，并保持普通消息不变；
- Chat、Responses 和 Claude 输入先归一为逻辑 turn，再执行一次 repair，避免三个适配器各有不同规则。

CB-N3 对 namespace 的验收与 KI-N1 不同：它验证 CodeBuddy 自身是否保留完整声明名以及返回形状，不自动套用 Kiro 的裸 child fallback。共享 transform 已覆盖 namespace 往返、超长稳定 hash、built-in 冲突和用户普通双下划线名称；CodeBuddy 专项 fixture 再覆盖三 Surface 进入上游前的声明/call/result 闭环。

CB-N4 在没有 CN 绑定账号 receipt 或冻结 vendor catalog 前，不把 deepseek-v4.1-flash 加入公开 capability。证据成立后也只对精确 native ID 设置模型专属字段；reasoning_summary=auto、verbosity=high 和 context window 不得泛化到所有 CodeBuddy 模型，且不得把 deepseek-v4.1-flash 重写为 deep-model 或其他 alias。

CB-N5 继续使用统一取消 token、CommitGuard 和 terminal 逻辑。专项测试已覆盖终态前截断、`[DONE]` 后无 EOF 和 marker 前后取消，结论为“通用实现已覆盖”，未复制 cli2api 的语言/框架特定代码。

CORE-N1 已新增 `src/proxy/providers/codebuddy/` facade，收拢精确 Account binding、canonical/model、站点目录 capability、payload preparation 与 generation fence；共享 forwarder 的 lease、usage、terminal、attempt budget 和一次 401 replay 决策保持不变。LIVE-N1 将 Intl/CN 拆为两份独立 schema-v2 私有 receipt：每份绑定 target commit/harness、Account 两代际、三 Surface Provider revision/runtime digest、Share revision/binding digest、exact model、三份 fresh `codebuddy_live_model_catalog` snapshot 与六个 body hash。fixture 只允许 `contract_verified/live_pending`；当前无真实 receipt，两站仍为 `null/live_pending`，也不自动开放 CB-N4、企业或多模态。

CodeBuddy reference delta 已迁为 append-only schema v2：`d81056c` 中原 schema-v1 文件 SHA-256、target commit/tree、全部 11 个历史字段的 canonical/逐字段 digest 均被冻结；新增 `cli2api@624874a` / tree `c2f02a3` committed-object snapshot，明确排除参考工作树的 `proxy.html`、`proxy.md`，以及 14 条不可变 observation，完整映射 CB-01～04、CB-N1～N5、CORE-N1、LIVE-N1、CB-R1～R3。每条记录绑定 source path/symbol/digest 与本仓库 committed target object；三条 reject 明确排除账号调度和跨边界 fallback、运营/商业控制面，以及未验证能力、外仓运行时依赖、伪 live 和缺唯一 terminal + EOF 仍成功。默认 audit 不读取外部仓库，只有显式 `--check-sources` 才复核冻结对象。

### 11.4 不采纳

不迁入每日签到、企业运营、domain fallback、账号池、prompt rewrite、未验证的企业/图像/视频能力，亦不通过隐藏 11148 原因来制造成功。

## 12. 横切增强

### 12.1 CORE-N1：渐进拆分巨型热路径

状态：completed；优先级：P2。Antigravity、Claude、Codex、Cursor、Grok、Kiro、Qoder、CodeBuddy 八个 Provider 切片均已完成，并分别在独立提交中先锁 golden、再迁移 orchestration、最后切调用点。CodeBuddy facade 持有精确 Account binding、canonical/model、站点目录 capability、payload preparation 与 generation fence；共享 forwarder 继续持有 lease、usage、terminal、attempt budget 和 401 replay 决策。这里关闭的是可维护性和审查边界，不改变 execution wire。

建议目标结构：

~~~text
src/proxy/providers/
  antigravity/
  claude/
  codex/
  cursor/
  grok/
  kiro/
  qoder/
  codebuddy/

src/proxy/execution/
  context.rs
  recovery.rs
  terminal.rs
  transport.rs
  memory.rs
  usage.rs
~~~

拆分采用“锁 golden → 纯移动 → 切一个调用点 → 删除旧分支”的顺序。每个 PR 只移动一个 Provider 或一个通用关注点，wire、错误类别、attempt 数、usage 和 metrics key 必须不变。Provider 方言、endpoint、特有 retry classifier 留在 Provider 模块；共享层不能再造一个巨型 enum switch。

state.rs 保留 ServerStateInner 作为跨域门面，逐步把 Account lifecycle、Provider runtime snapshot、Share mutation、Usage、cache/session 和 background scheduler 实现下沉。不得暴露内部锁或数据库连接给 proxy，也不得绕过现有域方法。

### 12.2 CORE-N2：统一请求生命周期内存预算

状态：partially_completed；优先级：P2。CX-N4 已以 `src/proxy/request_memory.rs` 落地 Codex HTTP/WS 请求生命周期预算，Antigravity 随后完成两条精确 Provider rail 的 HTTP 切片，Claude 完成精确 `claude_oauth` 的 body/压缩 SSE/retained stream-state 切片，Cursor 再完成双 rail、三 Surface 及 parked-session transfer 切片；Grok、Kiro、Qoder、CodeBuddy 仍待按各自 wire/owner 生命周期逐项验证。

预算对象绑定 request，不绑定 Account 全局静态值；至少核算：

- inbound raw/compressed 与解压后的 body；
- schema 和 canonical request 的有界膨胀；
- SSE/WS transport pending 与单 event；
- semantic bootstrap、replay snapshot 和 tool argument accumulator；
- normalized output 与 downstream backpressure queue；
- compaction/citation 等 Provider 专属临时结构。

每次 reserve/release 必须可审计，错误路径、取消和 fallback 后归零。超限错误是稳定、脱敏的本请求失败；不能通过切账号、切 Provider 或无限落盘绕过。指标只记录 budget class、阶段和大小桶。

### 12.3 EVID-N1：reference delta v2

状态：completed；优先级：P2。Antigravity 已用不可变 observation digest 固定 13 个 source delta、17 条处置记录及其本地测试映射；Claude 已固定 legacy v1 字段、13 个 source delta、两个 source snapshot、15 条处置记录；Codex 已固定六个 legacy v1 字段摘要、9 个 source delta、两个干净 source snapshot、13 条处置记录与 committed target object 映射；Cursor 已固定全部十个 legacy v1 字段摘要、一个明确排除 22 项工作树改动的 source snapshot、8 条处置记录及 committed target object 映射；Grok 已固定原 schema-v1 文件及十字段摘要、两个 source snapshot、10 条处置记录与 GR-R1 reject 边界；Kiro 已固定原 schema-v1 文件及十字段摘要、一个干净 source snapshot、13 条处置记录与 KI-R1～R3 reject 边界；Qoder 已固定原 schema-v1 文件及 11 字段摘要、一个干净 source snapshot、10 条处置记录与 QD-R1～R3 reject 边界；CodeBuddy 已固定原 schema-v1 文件及 11 字段摘要、一个明确排除两个未跟踪文件的 source snapshot、14 条处置记录与 CB-R1～R3 reject 边界。八类资产均为 append-only schema v2。

现有八个 assets/contract/*-reference-delta.json 已冻结上一轮证据。新一轮不得覆盖旧 source commit 后假装历史从未存在；应扩展为 append-only observation 或新 revision，至少记录：

- provider family、reference repo、commit、提交态 tree/hash；
- target baseline、观察日期、相关路径/符号；
- disposition：adopt、differential、live gate、reject；
- 对应 N 系列 ID、fixture ID、实现 commit 或拒绝理由；
- 外部工作树是否干净，以及只读 HEAD 的声明；
- 不含秘密的 source digest。

audit 必须验证 commit 格式、ID 唯一、目标文件存在、历史 observation 不被静默改写、每个 adopted gap 有测试映射。外部仓库不可成为 audit 运行时依赖；对象可选复核失败不能让本仓库离线构建失效。

### 12.4 LIVE-N1：统一真实验收队列

状态：live_pending；优先级：P1。本轮没有真实凭据输入，以下 rail 均未被 fixture 或参考项目结果错误提升。

待关闭的 rail：

- Antigravity：antigravity_oauth、agy_oauth、requestType 差分、可选 compaction；
- Claude：`oauth_inference`、`max_5x_plan`、`max_20x_plan`、`fable_5_1` 四个独立 operation；
- Codex：`gpt_image_2_5`、`gpt_image_2_5_flare`、`gpt_image_2_5_sunburst`、`ws_prewarm` 四个独立 operation；
- Cursor：OAuth、API-key；
- Grok：推理/媒体矩阵、remote compaction；
- Kiro：auth kind × region、可选 compact、多副本能力；
- Qoder：Global OAuth、Global PAT、CN OAuth；
- CodeBuddy：Intl、CN；企业和多模态仍不外推。

receipt 最少包含 provider/rail/site、目标 commit、harness revision、UTC 时间、model、Surface、stream 标记、请求形状标签、HTTP/协议终态、usage presence、refresh/retry/cooldown 决策、Provider/Account generation 和脱敏 body hash。禁止保存 Authorization、Cookie、token、opaque reasoning、prompt、图片、完整错误体、邮箱/uid 原值。

每条 rail 独立从 live_pending 升级；一条成功不能提升同 Provider 的另一站点、credential kind、模型或 operation。真实输入缺失时 readiness 应明确 blocked_inputs，而不是改写为成功。

### 12.5 已完成存储能力的维护边界

SQLite authority、迁移、备份和回滚不再列为新阶段。后续协议增强若增加持久状态，必须：

- 使用现有 repository transaction/generation CAS；
- 更新 schema/migration/backup manifest/rollback export 和故障注入；
- 默认 shadow 安全策略不回退；
- 不恢复 JSON 双写或请求路径同步整表写；
- 不手改 docs/provider/coverage.md。

## 13. 分阶段路线图

### Phase 0：冻结增量证据和失败形状

状态：本轮完成。八类推荐参考均固定提交态来源；有修改的工作树只读取 Git object；adopt/differential/live gate/reject 已映射到本地合同与测试。

| 顺序 | 工作 | 退出条件 |
| --- | --- | --- |
| 0.1 | 为全部 N 项建立 source commit/path/target symbol 映射 | 每项可追溯，not_adopted 也记录理由 |
| 0.2 | 新增最小自包含 fixture 与 differential harness case | 不在测试时读取外部仓库；预期缺口在当前 HEAD 确实红灯 |
| 0.3 | 冻结现有 wire/terminal/attempt/usage golden | 后续能区分有意修复与横向回归 |
| 0.4 | 建立 secret scanner 和 decoy upstream | fixture/差分产物无秘密；fail-closed case 证明零发网 |

若 fixture 在当前 HEAD 已经通过，应把项目从 confirmed_gap 降为“已有覆盖/仅补测试”，不得为了匹配参考代码形状修改实现。

### Phase 1：P0 协议与作用域修复

状态：本轮完成。下列顺序保留为实现审计记录；每项均有自包含 fixture，未引入跨账号/Provider/rail/site 恢复。

推荐切片顺序：

1. KI-N3 P0：Kiro Compact 明确发网前拒绝。
2. CX-N2：精确 metadata 清理。
3. CL-N1：Claude rate-limit scope 决策表。
4. CX-N1：Codex 启动 announcement 与 CommitGuard。
5. AG-N3：Gemini 目标 billing metadata 精确过滤。
6. CB-N2：reasoning-only 与逻辑 turn。
7. CB-N1：工具轮次 repair/11148。
8. KI-N1：唯一裸 namespaced tool 恢复。
9. AG-N1：grounding/citation 三 Surface 映射。

每个切片采用“失败 fixture → 最小生产修复 → mutation/property/stream 测试 → reference delta observation”的独立提交。不得把 CORE-N1 重构混入这些行为修复。

Phase 1 总退出条件：

- 所有 P0 fixture 由红转绿；
- 固定 Provider/Account/rail/generation 不变量无回归；
- 首个真实业务输出前后恢复边界可证明；
- tool、citation、reasoning、usage 无静默丢失或伪造；
- Kiro Compact 未获证据前零发网；
- 现有八类 golden 除批准差异外字节级或语义级稳定。

### Phase 2：P1 差分、能力与真实验收

状态：离线可判定项已完成；所有需要真实凭据的条目仍为 `live_pending`，没有用 mock 或参考项目结果替代本仓库 receipt。

| 组 | 工作 |
| --- | --- |
| Antigravity | AG-N2、AG-N4、AG-N5、AG-N6 |
| Claude | CL-N2、CL-N3、CL-N4 |
| Codex | CX-N3 |
| Cursor | CUR-N1 |
| Grok | GR-N2 |
| Kiro | KI-N2、KI-N3 P1 |
| Qoder | QD-N1 |
| CodeBuddy | CB-N3、CB-N4、CB-N5 |
| 横切 | LIVE-N1 |

Phase 2 只对差分红灯或真实 receipt 已证明的能力修改生产代码。没有凭据的项目继续保留 live_pending/runtime disabled；不得用参考仓库 live log 或 mock 代替本仓库 receipt。

### Phase 3：P2 架构、内存和证据

状态：部分完成。CX-N4/CORE-N2 的 Codex、Antigravity、Claude 与 Cursor 四个切片、八类 Provider 的 CORE-N1 与 EVID-N1、Qoder 与 CodeBuddy LIVE-N1 gate、CUR-N2 与 QD-N2 已完成；其余四个 Provider 的 memory 推广留待后续独立变更。

1. 先完成 CX-N4 的 request memory budget 并抽取 CORE-N2；Codex、Antigravity、Claude、Cursor 四个切片已完成，其余 Provider 逐项推进。
2. 按 Provider 分批实施 CORE-N1；八类 Provider 已全部完成。
3. 实施 EVID-N1 append-only reference delta v2。
4. 执行 CUR-N2、QD-N2 的周期复核。
5. 对 state.rs 和 server_sqlite.rs 做纯结构拆分，不重新设计 authority。

Phase 3 退出条件是依赖方向、锁顺序、wire golden、故障测试和存储 audit 全部通过；文件变短本身不是退出条件。

### Phase 4：P3 观察项

状态：GR-N1 已按 observation-only 完成；没有增加自动质量重试。

只实施 GR-N1 的脱敏诊断与经数据证明的低收益优化。任何自动质量重试都需要厂商明确错误信号、真实 receipt、同账号 pre-commit 约束和误报评估，否则保持 not_adopted。

## 14. 测试与发布门禁

### 14.1 通用命令

实现 PR 按风险至少执行：

~~~bash
cargo fmt --check
cargo check
cargo test
node scripts/audit/audit-server-provider-contract.mjs
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
node scripts/audit/audit-docs-index.mjs
scripts/static-checks.sh
scripts/smoke/smoke-local.sh
RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh
~~~

本轮包含生产代码、合同与审计修改，已运行完整门禁；以后只改本文档时才可缩减为 Markdown/diff 与 docs index，后续代码实施仍必须运行与风险相称的完整门禁。

### 14.2 本轮最终验证快照

| 门禁 | 2026-09-19 结果 | 判定 |
| --- | --- | --- |
| `RUST_MIN_STACK=67108864 cargo test --no-fail-fast` | lib 3141 passed/1 ignored；API contract 124 passed；两个独立 integration 各 1 passed；0 failed | 通过 |
| `scripts/static-checks.sh` | rustfmt、Clippy、JSON/Node/Shell、Provider/产品边界/依赖方向/文档审计全部通过；Provider audit 142 tests、smoke 13 tests、Web 41 files/213 tests 通过 | 通过 |
| 八类 reference delta `--check-sources` | Antigravity、Claude、Codex、Cursor、Grok、Kiro、Qoder、CodeBuddy 均按冻结 Git object 通过 | 通过；不构成 live receipt |
| `scripts/smoke/smoke-local.sh` | health、Web fallback、setup、密码/API Token 登录、Provider/Share 创建通过 | 通过 |
| `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` | `failures=0`，但因主动跳过内置测试产生 1 个 internal blocker；缺 8 个真实环境输入且 deployment 未测，共 9 个 blocker | `decision=blocked` / `blocked_inputs`，符合预期 |
| `git diff --check`、docs index | 无 whitespace 错误；文档索引完整 | 通过 |
| Antigravity 后续专项 | 67 个 Rust 关键词测试通过；13 个 Node 验收/环境门禁测试通过；13 个 source delta、17 条 v2 observation audit 通过 | CORE-N2 `fixture_verified`；两条 rail 仍为 `live_pending` |
| Claude 后续专项 | 249 个 Rust Claude 关键词测试通过；12 个 Node receipt gate 测试通过；13 个 source delta、15 条 v2 observation 默认及 `--check-sources` audit 通过 | CORE-N2 `fixture_verified`；四个 operation 仍为 `live_pending` |
| Codex 后续专项 | 389 个 Rust Codex 关键词测试通过；12 个 Node receipt gate 与 7 个付费探针安全测试通过；9 个 source delta、13 条 v2 observation audit 通过 | 通过；四个 operation 仍为 `live_pending` |
| Cursor 后续专项 | 304 个 Rust Cursor 关键词测试、16 个 request-memory 关键词测试通过；Clippy `-D warnings` 通过；7 个顶层 Node receipt gate/21 个断言保持通过；10 个 legacy 字段摘要、1 个 dirty-worktree-excluded snapshot、8 条 v2 observation 默认及 `--check-sources` audit 通过 | CORE-N2 `fixture_verified`；OAuth/API-key 两条 rail 仍为 `live_pending` |
| Grok 后续专项 | 8 个 Rust replay/HTTP/WS 专项通过；6 个顶层 Node receipt gate/11 个断言通过；10 个 legacy 字段摘要、2 个 committed-object snapshot、10 条 v2 observation 的默认与 `--check-sources` audit 均通过 | 通过；inference/media/remote_compaction 三个 operation 仍为 `live_pending` |
| Kiro 后续专项 | 97 个 Rust Kiro 关键词测试通过；5 个顶层 Node receipt gate 覆盖八个 scope；10 个 legacy 字段摘要、1 个干净 committed-object snapshot、13 条 v2 observation 与 3 条 reject 边界的默认及 `--check-sources` audit 均通过 | 通过；八个 auth-kind × region receipt、remote compaction 与 shared cache 仍为 `live_pending`/disabled |
| Qoder 后续专项 | 65 个 Rust Qoder 关键词测试通过；9 个 Node oracle mutation 与 7 个三 rail loopback harness fixture 通过；append-only reference delta v2 默认审计及 TokenRouter committed-object `--check-sources` 通过 | 通过；Global OAuth、Global PAT、CN OAuth 仍各自为 `live_pending` |
| CodeBuddy 后续专项 | 55 个 Rust CodeBuddy 关键词测试、6 个双站 receipt fixture 与 11 个环境门禁测试通过；11 个 legacy 字段摘要、1 个 dirty-worktree-excluded snapshot、14 条 v2 observation 与 3 条 reject 边界的默认及 `--check-sources` audit 均通过 | 通过；Intl/CN 两站 receipt 与 CB-N4 仍为 `live_pending`/disabled |

正式测试使用 64 MiB `RUST_MIN_STACK`；默认 2 MiB 线程栈的既有溢出不作为本轮回归。离线 readiness 中的 `local-contracts-unverified` 仅表示该命令显式设置了 `RUN_TESTS=0`，不能覆盖上表已独立完成的全量测试；同样也不能消除真实凭据与部署 blocker。没有任何 rail 因 fixture、参考项目结果或本地 smoke 被提升为 `live_verified`。

### 14.3 专项测试矩阵

| 范围 | 必测内容 |
| --- | --- |
| Grounding/citation | Unicode offset、重复/缺失 chunk、三 Surface、流/非流、事件顺序、terminal |
| Rate-limit scope | header 缺失/冲突、utilization、overage、model、org reason、reset/Retry-After、generation CAS |
| Responses semantics | 空 announcement、未知 item/part、server operation、overload 前后、HTTP/SSE/WS/Lite |
| Metadata sanitizer | 顶层命中、嵌套同名字段、arguments、幂等、mutation |
| Tool repair/namespace | 完整/中断/孤立/重复/歧义、超长名、分块参数、三 Surface |
| Kiro fallback | 精确 400/401/403/429/5xx、timeout/TLS/decode、host、region、auth kind |
| Cancellation | terminal 前后断连、下游取消、上游取消、资源释放、无错误 outcome 反记 |
| Memory budget | 每阶段 reserve/release、压缩膨胀、fallback、取消、背压、并发隔离 |
| Live receipt | rail/site/model/operation 独立，secret scan，通过与 blocked_inputs 都可审计 |

### 14.4 差分判定纪律

- 比较协议语义，不要求不同 Surface 产生相同 JSON。
- 参考实现与厂商证据冲突时，以厂商证据和本仓库真实 receipt 为高优先级。
- 两个成熟参考互相冲突时保持当前安全行为，直到 live receipt 判定。
- 差分绿灯只补测试和 evidence，不做无意义代码同构。
- 差分红灯后只修该失败形状，并用 decoy/mutation 防止规则扩大。

### 14.5 发布阻断条件

出现以下任一情况不得提升 capability 或 live 状态：

- 需要真实凭据却只有 fixture/mock；
- retry 能越过 Account、Provider、rail 或身份代际；
- post-commit 仍可能透明 replay；
- 新 sanitizer 递归删除用户内容；
- tool/citation/reasoning 被静默丢弃或伪造；
- compact/新模型在无专属合同下走普通生成；
- receipt 或日志含秘密；
- registry、生成 coverage、UI matrix 和生产入口不一致。

## 15. 证据索引

### 15.1 目标代码锚点

| 主题 | 目标路径/符号 |
| --- | --- |
| Antigravity requestType/search model | src/proxy/adapters.rs 的 Antigravity request builder |
| billing metadata 与 Gemini system | src/proxy/transforms.rs 的 strip_leading_anthropic_billing_header、anthropic_system_to_gemini |
| tool turn 清理 | src/proxy/transforms.rs 的 drop_incomplete_anthropic_tool_turns |
| Claude quota/429 | src/proxy/claude_quota_headers.rs、src/proxy/forwarder.rs 的 classify_claude_rate_limit |
| Codex 提交语义 | src/proxy/response_semantics.rs 的 classify_value |
| WS/transport 恢复 | src/proxy/execution、src/proxy/forwarder.rs |
| Kiro namespaced tool | src/proxy/kiro.rs 的 map_tool_name/original_tool_name、src/proxy/kiro/tool_bridge.rs |
| Kiro usage fallback | src/clients/oauth/kiro_device.rs 的 fetch_usage_limits |
| Kiro Compact 路由 | src/proxy/forwarder.rs 的 forward_claude_kiro |
| CodeBuddy payload | src/proxy/codebuddy_runtime.rs 的 build_codebuddy_payload、codebuddy_message_is_semantic |
| SQLite authority | src/repository/server_sqlite.rs、docs/architecture/storage.md |

### 15.2 外部增量提交

Antigravity：

- CLIProxyAPI ef63d2e7、7fcbdf88：web search、grounding 与 citation。
- CLIProxyAPI a9e92b81：request model metadata。
- CLIProxyAPI b681a1e0、8c984672、fd3e6623：中途 system、孤立 output、空 text block。
- Antigravity-Manager 734e2bde、9fd77989：动态 requestType 与 billing metadata。

Claude：

- CLIProxyAPI 44eaef00：model/overage scope。
- CLIProxyAPI 7c32971b、2bcebaa8：terminal-after-disconnect 与 tool pairing。
- CLIProxyAPI 377c315f、75ce6352、fc96a87f：fingerprint、CAQS v4、organization identity。
- CLIProxyAPI 2e6b1d83：Claude-compatible thinking replay 的 session/turn/block/进程容量、TTL 与 CAS 边界；只作为 CORE-N2 差分信号。

Codex：

- CLIProxyAPI e696ea47、f702bc1a：Turn-State、Responses Lite。
- CLIProxyAPI cb73cd99、6e307553、b5ba02c2：bootstrap buffering 与 WS pong starvation。
- CLIProxyAPI 4311ae87：per-model search capability。
- codex2api dc47d131、19ee8db4：精确 metadata stripping 与 request memory budget。

Cursor：

- OmniRoute@02c663cdd0e85 提交态；CUR-N2 复核无新增 wire，工作树修改不作证据。既有 16 MiB frame ceiling、rolling-buffer splice、parked session TTL/数量/close cleanup 只作为 CORE-N2 retained-state 差分信号，不证明统一 budget 或 gzip expansion。

Grok：

- grok2api@7f3f3d3c：visible/plaintext-ratio 质量重试，仅作不采纳评估。
- sub2api@ab99d56e9626e 提交态；工作树修改不作证据。

Kiro：

- kiro.rs 3219d1c、db3e912：region 与 context_window，目标已有等价实现。
- kiro.rs f413e7d、3194bb2：裸 namespaced tool 与 profileArn fallback。
- kiro.rs 0b8c7de、d62054f、13763b6：remote compaction。

Qoder：

- TokenRouter@7faf9469bc695 的 qoder_gateway_handler.go 增量；无专项 wire 变化。
- 本仓库 assets/contract/qoder-cli-oracle.json 继续是高于兼容参考的第一来源。

CodeBuddy：

- cli2api cdc80d6：中断工具轮次/11148。
- cli2api a9ae393：namespace/reasoning。
- cli2api 6ac6f83、d361559、eef5b2c：deepseek-v4.1-flash。
- cli2api 76c4dab：stream cancellation。

## 16. 完成定义

本增强计划完成需要同时满足：

1. 所有 P0 confirmed_gap 都有来源、当前失败 fixture、最小修复和回归测试；若初始 fixture 已绿，记录降级结论而不强改。
2. CL-N1 不再把 overage/model/含糊 429 污染为 Account cooldown，且 generation fence、reset 上限和同账号边界不退化。
3. CX-N1 只有已知且语义为空的 announcement 不提交；未知业务事件仍立即提交，四种 transport 行为一致。
4. AG-N1 在三 Surface、流/非流和 Unicode 上守恒 grounding/citation；AG-N3 不误删普通 system 文本。
5. KI-N1 对唯一裸 child 可恢复、歧义 fail closed；KI-N3 在未获真实证据前对 Compact 零发网。
6. CB-N1/CB-N2 不再因中断轮次或 reasoning-only 丢失触发已冻结失败，同时不伪造工具结果。
7. differential_first 项都有“已有覆盖、实施修复或继续 live gate”的明确结论，不无限悬置。
8. 每条 live rail 独立验收；无真实输入的能力继续诚实标记 live_pending/runtime disabled。
9. Provider/Share/Account 固定绑定、pre-commit 恢复、总预算和秘密保护均无回归。
10. SQLite authority、迁移、备份与回滚保持既有实现，不被重复建设或退回 JSON 双写。
11. registry、合同源、生成 coverage、UI matrix、PROTOCOL_EVIDENCE 和 reference delta 一致；docs/provider/coverage.md 未被手改。
12. 外部仓库没有进入构建、测试、CI、发布或运行时依赖。

当前判定：第 1～7、9～12 项已在本地范围内满足；第 8 项仍按 LIVE-N1 保持 `live_pending`，因为没有提供真实凭据、Router/Share 环境和部署输入。八类 Provider 已完成 CORE-N1 与 EVID-N1；CORE-N2 已完成 Codex、Antigravity、Claude、Cursor 四个切片，向 Grok、Kiro、Qoder、CodeBuddy 的推广仍是明确后续项。因此可以关闭首轮静态差分、P0/P1 离线实施、八类 Provider lifecycle 拆分及证据迁移，不能把完整真实验收队列或整体架构计划标记为完成。
