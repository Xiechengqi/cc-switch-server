# cc-switch-server 反代增强增量分析与实施计划

> 文档性质：八类反代的增量差异分析与实施路线图，不是架构或协议真值。架构以 docs/architecture/overview.md 为准，Provider 身份与能力以 assets/contract/provider-registry.json 为准，wire 证据以 PROTOCOL_EVIDENCE.md、厂商材料和本仓库冻结 fixture 为准。
>
> 分析日期：2026-09-18，实施状态更新至 2026-09-28。分析起点：`origin/main@7c9ef35`；Provider 生产差分起点为 `4712ca063930fea507910c37749595c8d6acc073`。首轮生产行为与协议证据冻结到 `db65188`，验证门禁收口到 `248e7c2`。后续已按 Provider 完成 Antigravity（`e5bfc34`）、Claude（`6b0a0fe`）、Codex（`069f3ef`）、Cursor（`8f72bb9`）、Grok（`269850a`）、Kiro（`1124082`）、Qoder（`2cc8a01`）与 CodeBuddy（`d81056c`）的首批 CORE-N1/LIVE-N1 切片，并完成八类 EVID-N1 迁移；CORE-N2 最后由 Qoder（`3485571`）和 CodeBuddy（`573dc47`）收口。Phase 3 的本地结构项由 SQLite 关注点拆分（`7f2c531`）、state 并发准入拆分（`4237756`）和后台调度拆分（`a0ec1ea`）收口。第二轮已完成 Antigravity AG-N7～N11（`aeeaf1a`）、Claude CL-N5～N6（`7794033`）、Codex CX-N5～N12（`f890774`）和 Grok GR-N3（`a0f567a`）；Cursor 与 Kiro 第二轮均冻结为 `reviewed_no_wire_delta`，GR-N4 因缺固定账号证据保持 `live_pending` 且不改 wire。其余第二轮 Provider 仍按第 17 节状态推进。
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
| `2eca08a` | Grok CORE-N2：精确 OAuth binding 的 sticky budget 覆盖 HTTP/SSE/WS reasoning replay、retained state、媒体解码/规范化与图片流；容量耗尽不重试或记为网络故障 |
| `7526159` | Kiro CORE-N2：精确 Kiro/Amazon Q binding 的 sticky request-memory budget 覆盖 canonical/wire、图片工作集、EventStream 与下游 transform；容量耗尽稳定终止且不重放或记为网络故障 |
| `3485571` | Qoder CORE-N2：三条 credential rail、三 Surface 的 sticky request-memory budget 覆盖 canonical/runtime/catalog/payload、COSY 编码签名、响应/decoder/aggregator 与下游 transform；容量耗尽不重放且按 capacity shed 分类 |
| `573dc47` | CodeBuddy CORE-N2：Intl/CN、三 Surface 的 sticky request-memory budget 覆盖 canonical/runtime/catalog/payload、JSON wire/headers、响应/decoder/aggregator 与下游 transform；容量耗尽不重放、不刷新且按 capacity shed 分类 |
| `7f2c531` | SQLite repository 纯结构拆分：schema、加密 payload 投影/秘密校验和 WAL envelope/checksum 下沉到私有模块；authority、SQL、错误文本和外部 API 不变 |
| `4237756` | state 并发准入纯结构拆分：Share/Account in-flight tracker、RAII guard、快照和容量错误下沉，原 `crate::state` 公共路径不变 |
| `a0ec1ea` | state 后台调度纯结构拆分：备份、升级、审计上传、Router 心跳、公网 IP、Share 重试和 Account refresh 循环下沉；锁、持久化与 wire 不变 |

| 类别 | 本轮已关闭 | 仍保持门禁/后续计划 |
| --- | --- | --- |
| Antigravity | AG-N1～N5、CORE-N2 `fixture_verified`；CORE-N1 Provider 切片与 reference-delta v2 已完成 | AG-N6 与两条 OAuth rail `live_pending`；compaction 继续关闭 |
| Claude | CL-N1～N4 与 CORE-N2 `fixture_verified`；CORE-N1 Provider 切片与 reference-delta v2 已完成 | OAuth inference、Max 5x/20x plan、Fable 5.1 四个 operation 独立 `live_pending` |
| Codex | CX-N1～N4 `fixture_verified`；CORE-N1 Provider 切片、CORE-N2 首个 request-memory 切片与 reference-delta v2 已完成 | GPT Image 2.5 三个 exact-model operation 与 WS prewarm 独立 `live_pending` |
| Cursor | CUR-N2 `reviewed_no_wire_delta`、CORE-N2 `fixture_verified`；CORE-N1 Provider facade 与 reference-delta v2 已完成 | CUR-N1 OAuth/API-key 两 rail 独立 `live_pending` |
| Grok | GR-N1 `fixture_verified`（观察-only）、CORE-N2 `fixture_verified`；CORE-N1 Provider lifecycle 与 reference-delta v2 已完成 | GR-N2 inference/media/remote_compaction 三个 operation 独立 `live_pending` |
| Kiro | KI-N1/N2 `fixture_verified`；KI-N3 fail-closed fixture；CORE-N1 Provider facade、CORE-N2 与 reference-delta v2 已完成 | KI-N3 启用与 auth kind × region receipt `live_pending`；KI-05 shared cache 继续关闭 |
| Qoder | QD-N2 `reviewed_no_wire_delta`、CORE-N2 `fixture_verified`；CORE-N1 Provider facade 与 reference-delta v2 已完成 | QD-N1 三 rail 独立 `live_pending` |
| CodeBuddy | CB-N1/N2 `fixture_verified`；CB-N3/N5 共享实现已覆盖；CORE-N1 Provider facade、CORE-N2、Intl/CN schema-v2 私有 receipt gate 与 reference-delta v2 已完成 | CB-N4 与两站真实 receipt `live_pending`，v4.1 runtime disabled |

CORE-N2、CORE-N1 与 EVID-N1 均已完成八个 Provider 切片，Phase 3 的 `state.rs` / `server_sqlite.rs` 本地纯结构拆分也已完成。所有缺真实凭据的 LIVE-N1 operation/rail/site 继续保持门禁；本地容量合同或结构验收完成不等于任何真实订阅状态提升。

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

OmniRoute 从旧冻结点到 `02c663cdd0e85`，以及第二轮从该点到 `443d66996d69`，均没有新的 Cursor wire、protobuf、auth 或 session 专项提交。目标已经具备 OAuth/API-key 双 rail、protobuf/open-sse、session/continuation、模型目录和自包含 fixtures，因此 CUR-N2 没有发现需要新增生产 wire 的 confirmed_gap；后续实施的 CORE-N2 是独立横切容量治理，不改变该结论。

OmniRoute 工作树有 22 项本地修改，全部排除在证据之外。后续只能把新的已提交 Cursor 变更纳入 reference delta；不能引用工作树文件、截图或本地运行结果。

### 7.2 新增项

| ID | 状态 | 优先级 | 工作与退出条件 |
| --- | --- | --- | --- |
| CUR-N1 | live_pending | P1 | OAuth 与 API-key rail 仍需分别产生本仓库 receipt；一条成功不得外推另一条 |
| CUR-N2 | reviewed_no_wire_delta | P2 | 已复核 OmniRoute@443d6699；14 个 committed changed path 均非 Cursor wire，六个 wire 对象无变化，22 项工作树修改排除，生产代码不变 |

CUR-N1 继续保持 Account 固定绑定。credential kind、endpoint、session identity 和 model catalog 必须写入脱敏 receipt 的结构字段；token、machine identity 原值和 prompt 不得落盘。

`8f72bb9` 已以 `src/proxy/providers/cursor/` 建立共享 forwarder 与既有 Cursor 协议实现之间的生命周期 facade，收敛 adapter、模型选择、native dispatch 和 h2 timeout mapping；binding、lease、Share、attempt、terminal、usage、protobuf 与 session 所有权不变。OAuth/API-key receipt 升为 schema v2/harness revision 2，分别绑定当前 target commit、Provider/runtime、Share、credential generation、精确 `*-fast` model、22 项检查、10 份 body hash、5 项测量、固定恢复决策与 decoy/secret scan；仓库外真实文件要求 `0600`。没有真实输入时两条 rail 仍为 `live_pending`。

`3e0f5ff` 完成 CORE-N2 的 Cursor 切片：只对精确 `cursor_oauth` / `cursor_apikey` binding 启用 sticky budget，并同时覆盖 Claude、Codex、Gemini Surface。最终 normalized body 在发网前计入；Agent plan/JSON、工具 schema、图片、protobuf、h2 write queue/parser、gzip expansion、decoded/pending frame、错误 body、SSE/JSON 聚合、semantic retry 与 completed-response replay clone 均共享同一请求预算。OAuth 401 和 semantic recovery 在耗尽后返回 `DeniedBudget`，不发起 replay。

parked session 会保留 request reservation；continuation 先向新请求预算预留 session、parser 与 pending frame，再释放旧预算，close、expiry 与失败 guard 均清理 retained state。四种已提交 SSE Surface 使用稳定 `cc_switch_request_memory_exhausted` 终止码，usage 归类 `memory_capacity`，Provider outcome 归类 capacity shed。专项验证为 Cursor 304/304、request-memory 16/16、Clippy `-D warnings` 通过。

Cursor reference delta 已迁为 append-only schema v2：原 schema v1 的十个历史字段逐项由 digest 固定；两个明确排除 22 项工作树改动的 OmniRoute committed-object snapshot 分别冻结 `02c663cd` 与 `443d6699`，`CUR-N2-2026-09-28` review extension 固定其 14 个 committed changed path 和六个零变化 Cursor wire path。当前共有 8 条不可变 observation，完整映射 CUR-01～03、CUR-N1/N2、CORE-N1、CORE-N2、LIVE-N1。`CUR-OBS-0008` 只把参考的 16 MiB frame ceiling、rolling-buffer splice、5 分钟/100 session 与 close cleanup 作为“retained state 应有界”的差分信号；`evidenceExtensions` 记录 `CORE-N2-CURSOR=fixture_verified`。每条记录绑定 source commit/tree/path/symbol/digest 与本仓库 committed target baseline/implementation object；默认审计不读取外部仓库，只有显式 `--check-sources` 才复核冻结对象。

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
| CORE-N2 | fixture_verified | P2 | 精确 Grok OAuth 的 HTTP/SSE/WS/media 已纳入 sticky request-memory budget；耗尽稳定终止且不 replay、换号或记为普通网络故障 |

GR-N1 指标只能记录低基数 outcome kind、是否有 terminal/tool/visible text 和有界长度桶，不能记录 plaintext、reasoning 或比例对应的原文。如果未来厂商提供明确错误码或 receipt 证明某形状必然失败，也只能在同账号、pre-commit、总预算内 fail closed 或重试一次；不能换账号。

### 8.3 不采纳

明确不采纳 visible token 阈值、plaintext-ratio 阈值、内容关键词或“看起来不像回答”驱动的生产重试，也不采纳账号池和跨账号 balance failover。

`269850a` 已把 Grok replay scope 派生、snapshot ownership、CAS 清理/提交与 Provider/Account/Share generation fence 收敛到 `src/proxy/providers/grok/`；HTTP/WS wire、固定绑定、共享 attempt/10 秒预算、一次 pre-commit 明确拒绝恢复和零 post-commit replay 均未改变。LIVE-N1 同时新增 inference、media、remote_compaction 三个独立私有 receipt gate，绑定当前 target commit、Provider/runtime、Account auth/token generation、Share revision、签名用户、精确 model/session/turn、fresh catalog、checks/body hashes/measurements、恢复决策、decoy counters 与 secret scan；真实文件必须位于仓库外且为 `0600`。`grok-oauth-real.mjs` 仍是 `probe_only/live_pending`，当前三份 receipt 均为 `null`。

Grok reference delta 已迁为 append-only schema v2：`269850a` 中原 schema-v1 文件 SHA-256、target commit/tree、全部十个历史字段的 canonical/逐字段 digest 均被冻结；新增 grok2api 干净 snapshot、明确排除 6 项工作树修改的 sub2api snapshot，当前共有 11 条不可变 observation，完整映射 GR-01～05、GR-N1、CORE-N1、CORE-N2、LIVE-N1、GR-R1。每条记录绑定 source commit/tree/path/symbol/digest 与本仓库 committed target object；GR-R1 将启发式自动重试、账号池/轮换、商业复合路由/计价和 soft quota gate 固定为 reject，默认审计不读取外部仓库。

`2eca08a` 完成 CORE-N2 的 Grok 切片。grok2api 的 committed reasoning cache 只提供 4096 entry、30 分钟 TTL、scope key 与 LRU 淘汰信号，没有单 proof 字节上限或统一请求预算；本仓库因此独立把精确 `grok_oauth` binding 的 HTTP JSON/SSE、Responses WebSocket turn 与 media 全部接入 sticky budget。replay cache 在 proof clone 前 reserve，request parse/apply、capture、stream accumulator、retry payload 与 binding drift 重新准备共享同一预算；SSE completed-search state 固定容量并摘要超长 ID。

媒体路径在解压前建立预算，合计 raw/decoded/normalized body、multipart/base64 扩张、响应 wire/解压 body 和图片 JSON/SSE stream buffer。HTTP 未提交时返回稳定 503，已提交 SSE 发送同码 terminal，WS 发送 error 后 Close；容量耗尽取消当前上游，不再 reasoning recovery、WS→HTTP fallback、换 Account/Provider/rail，也不记为 `NetworkFailure`。完整 lib 在默认测试线程栈上 3153 passed/1 ignored；Grok 关键词 184/184、Clippy `-D warnings`、rustfmt 与 diff 检查通过。该结果仅支持 `CORE-N2-GROK=fixture_verified`，不改变 GR-N2 三个 operation 的 `receipt=null/live_pending`，remote compaction 仍为 runtime disabled。

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
| CORE-N2 | fixture_verified | P2 | 精确 Kiro/Amazon Q binding 的 canonical/wire、图片工作集、EventStream retained state 与下游 transform 纳入 sticky request-memory budget |

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

`7526159` 完成 CORE-N2 的 Kiro 切片。精确 `kiro_oauth` / `amazon_q_oauth` binding 在 Claude Messages、Codex Chat Completions/Responses 的非流与流式路径共享一个 sticky request budget；raw/decoded body、canonical JSON、prepared request、序列化 wire、目标 headers、prompt-cache 临时状态、非流响应读取/解压/EventStream 聚合/转换，以及 decoder/frame、tool JSON、tool leak filter、SSE builder、usage parser 与 downstream transform 全部纳入核算。图片在 base64 decode 前预留 upper bound，并核算像素 decode、resize、RGB、JPEG 与重新 base64 的工作集。

同账号 401 replay 前后预算不重置；耗尽后未提交请求稳定返回 `503 / cc_switch_request_memory_exhausted`，已提交 stream 发同码 terminal，usage 标为 `memory_capacity`，Provider outcome 为 capacity shed 而不是 `NetworkFailure`。专项验证为 Kiro 102/102、request-memory 21/21、Amazon Q 14/14，Clippy `--all-targets -D warnings` 通过。该结果不提升八路真实 receipt，也不启用 remote compaction 或 shared cache。

Kiro reference delta 已迁为 append-only schema v2：`1124082` 中原 schema-v1 文件 SHA-256、target commit/tree、全部十个历史字段的 canonical/逐字段 digest 均被冻结；新增干净的 `kiro.rs@be0c042` / tree `5e656c1` committed-object snapshot，以及 14 条不可变 observation，完整映射 KI-01～05、KI-N1～N3、CORE-N1/N2、LIVE-N1、KI-R1～R3。`KI-OBS-0014` 只把参考的 16 MiB frame/buffer ceiling 作为“retained state 应有界”的差分信号，不把它误写为统一生命周期预算、图片膨胀或下游 transform 的证明；`CORE-N2-KIRO=fixture_verified` 绑定本仓库 committed implementation object。每条记录绑定 source commit/tree/path/symbol/digest 与本仓库 committed target object；三条 reject 明确排除跨 Account/auth-kind/region/Provider 或宽 profile/host fallback、Redis/session-affinity/cache 路由，以及无独立执行器与真实证据的 remote-compaction 自动启用。默认审计不读取外部仓库，只有显式 `--check-sources` 才复核冻结对象。

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

`3485571` 完成 CORE-N2 的 Qoder 切片。精确 `qoder_cosy` binding 在 Global OAuth、Global PAT、CN OAuth 三条 rail 与 Claude、Codex、Gemini 三个 Surface 上共享同一 sticky request budget；同账号 pre-commit 401 recovery 前后不重置。预算覆盖 canonical JSON、runtime/catalog clone、prepared payload、COSY plain JSON 与三份编码工作副本、签名 preimage/headers、错误响应、SSE chunk、decoder buffer/canonical event、非流 text/reasoning/tool arguments 聚合，以及三个下游 bridge 的 retained state。最终响应和流 chunk 的 reservation 跟随 owner 生命周期释放。

容量耗尽统一返回 `503 / cc_switch_request_memory_exhausted`，usage 标为 `memory_capacity`，Provider outcome 为 `CapacityShed`；不 replay、不 refresh、不记为 `NetworkFailure`，已提交流只输出一次脱敏 terminal。专项验证为 Qoder 74/74、request-memory 24/24，Clippy `--all-targets -D warnings` 通过。该切片不提升三条真实 rail receipt。

Qoder reference delta 已迁为 append-only schema v2：`2cc8a01` 中原 schema-v1 文件 SHA-256、target commit/tree、全部 11 个历史字段的 canonical/逐字段 digest 均被冻结；新增干净的 `TokenRouter@7faf946` / tree `b205061c` committed-object snapshot，以及 11 条不可变 observation，完整映射 QD-01～03、QD-N1/N2、CORE-N1/N2、LIVE-N1、QD-R1～R3。`QD-OBS-0011` 只把参考的 4 MiB 控制面读取上限、显式 SSE line ceiling 与非流事件 retained array 作为“retained state 应有界”的差分信号；参考默认 line ceiling 为 500 MiB，并不证明统一 sticky request budget、COSY 编码膨胀、下游 transform 或容量分类。`CORE-N2-QODER=fixture_verified` 绑定本仓库 committed implementation object。每条记录绑定 source commit/tree/path/symbol/digest 与本仓库 committed target object；三条 reject 明确排除账号池/轮换/affinity 与跨 Account/rail/site/Provider fallback、商业计费/Key/渠道/维护调度/多租户后台，以及外仓运行时依赖、fixture/参考成功冒充 live 和缺唯一 terminal + EOF 仍成功。默认审计不读取外部仓库，只有显式 `--check-sources` 才复核冻结对象。

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

`573dc47` 完成 CORE-N2 的 CodeBuddy 切片。精确 `codebuddy_oauth` binding 在 Intl/CN 两站与 Claude、Codex、Gemini 三个 Surface 上共享一个 sticky request budget，同账号 pre-commit 401 refresh/replay 前后不重置。预算覆盖 canonical JSON、runtime/catalog、prepared payload、JSON wire、请求 ID、目标 headers、错误 body/解析工作集、SSE transport chunk/rolling buffer/event parse/canonical `Bytes` owner、非流 content/reasoning/tool-call 聚合与最终 `Value`，以及三个下游 bridge 的 retained state；reservation 跟随最终响应/chunk owner 生命周期释放。

容量耗尽统一返回 `503 / cc_switch_request_memory_exhausted`，usage 标为 `memory_capacity`，Provider outcome 为 `CapacityShed`；不 replay、不 refresh、不记为 `NetworkFailure`，已提交流只输出一次脱敏 terminal。分发边界堆化 `forward_codebuddy` future，使默认 Tokio 测试线程栈无需额外 `RUST_MIN_STACK`。专项验证为 CodeBuddy 66/66、request-memory 26/26、memory-exhaustion 15/15，Clippy `--all-targets -D warnings` 通过；该切片不提升 Intl/CN 两站真实 receipt。

CodeBuddy reference delta 已迁为 append-only schema v2：`d81056c` 中原 schema-v1 文件 SHA-256、target commit/tree、全部 11 个历史字段的 canonical/逐字段 digest 均被冻结；新增 `cli2api@624874a` / tree `c2f02a3` committed-object snapshot，明确排除参考工作树的 `proxy.html`、`proxy.md`，以及 15 条不可变 observation，完整映射 CB-01～04、CB-N1～N5、CORE-N1/N2、LIVE-N1、CB-R1～R3。`CB-OBS-0015` 只把参考的 1/16 MiB body 读取上限、16 MiB SSE 单行 ceiling 与非流 retained aggregation 作为“读取、单行和聚合状态应有界”的差分信号；它不证明统一 sticky lifecycle budget、payload/header 副本、下游 transform、容量分类或恢复语义。`CORE-N2-CODEBUDDY=fixture_verified` 绑定本仓库 committed implementation object。每条记录绑定 source path/symbol/digest 与本仓库 committed target object；三条 reject 明确排除账号调度和跨边界 fallback、运营/商业控制面，以及未验证能力、外仓运行时依赖、伪 live 和缺唯一 terminal + EOF 仍成功。默认 audit 不读取外部仓库，只有显式 `--check-sources` 才复核冻结对象。

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

`4237756` 已把 Share/Account 请求并发准入、RAII guard、负载快照和容量错误下沉到 `src/state/in_flight.rs`，保持 `crate::state` 的公共类型路径不变；`a0ec1ea` 随后把周期备份、升级状态、审计上传、Router 心跳、公网 IP、Share 重试、quota/native token refresh 等循环下沉到 `src/state/background.rs`。`ServerStateInner` 仍是跨域门面，子模块没有直接持有数据库连接，也没有新增对存储锁的 `.write().await`，原锁顺序、持久化方法、调度间隔、重试和 wire 均未改变。

### 12.2 CORE-N2：统一请求生命周期内存预算

状态：completed；优先级：P2。CX-N4 已以 `src/proxy/request_memory.rs` 落地 Codex HTTP/WS 请求生命周期预算，Antigravity 随后完成两条精确 Provider rail 的 HTTP 切片，Claude 完成精确 `claude_oauth` 的 body/压缩 SSE/retained stream-state 切片，Cursor 再完成双 rail、三 Surface 及 parked-session transfer 切片，Grok 完成精确 `grok_oauth` 的 HTTP/SSE/WS/media 切片，Kiro 完成精确 `kiro_oauth` / `amazon_q_oauth` 的 canonical/wire、图片与 EventStream retained-state 切片，Qoder 完成三 rail、三 Surface 的 canonical/COSY/decoder/transform 切片，CodeBuddy 最后完成 Intl/CN、三 Surface 的 canonical/wire/header/decoder/aggregator/transform 切片。八类 Provider 的 CORE-N2 均已完成本地 `fixture_verified`，真实 receipt 状态不随之提升。

预算对象绑定 request，不绑定 Account 全局静态值；至少核算：

- inbound raw/compressed 与解压后的 body；
- schema 和 canonical request 的有界膨胀；
- SSE/WS transport pending 与单 event；
- semantic bootstrap、replay snapshot 和 tool argument accumulator；
- normalized output 与 downstream backpressure queue；
- compaction/citation 等 Provider 专属临时结构。

每次 reserve/release 必须可审计，错误路径、取消和 fallback 后归零。超限错误是稳定、脱敏的本请求失败；不能通过切账号、切 Provider 或无限落盘绕过。指标只记录 budget class、阶段和大小桶。

### 12.3 EVID-N1：reference delta v2

状态：completed；优先级：P2。Antigravity 已用不可变 observation digest 固定 13 个 source delta、17 条处置记录及其本地测试映射；Claude 已固定 legacy v1 字段、13 个 source delta、两个 source snapshot、15 条处置记录；Codex 已固定六个 legacy v1 字段摘要、9 个 source delta、两个干净 source snapshot、13 条处置记录与 committed target object 映射；Cursor 已固定全部十个 legacy v1 字段摘要、一个明确排除 22 项工作树改动的 source snapshot、8 条处置记录及 committed target object 映射；Grok 已固定原 schema-v1 文件及十字段摘要、两个 source snapshot、11 条处置记录与 GR-R1 reject 边界；Kiro 已固定原 schema-v1 文件及十字段摘要、一个干净 source snapshot、14 条处置记录与 KI-R1～R3 reject 边界；Qoder 已固定原 schema-v1 文件及 11 字段摘要、一个干净 source snapshot、11 条处置记录与 QD-R1～R3 reject 边界；CodeBuddy 已固定原 schema-v1 文件及 11 字段摘要、一个明确排除两个未跟踪文件的 source snapshot、15 条处置记录与 CB-R1～R3 reject 边界。八类资产均为 append-only schema v2。

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

`7f2c531` 已将固定 schema、加密 legacy payload 投影/秘密校验和 WAL envelope/checksum 校验分别下沉到 `src/repository/server_sqlite/{schema,payload,wal}.rs`。主 repository 继续独占 authority 状态机、transactional writer、backup/restore 和连接策略；SQLite contract audit 会分别验证主模块声明及三个权威子模块的锚点，避免拆分后靠文本拼接形成假绿。

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

状态：本地可实施项完成。CX-N4/CORE-N2、CORE-N1 与 EVID-N1 的八类 Provider 切片、Qoder 与 CodeBuddy LIVE-N1 gate、CUR-N2、QD-N2，以及独立的 `state.rs` / `server_sqlite.rs` 纯结构拆分均已完成；只剩不能靠离线输入关闭的 LIVE-N1 真实验收。

1. 先完成 CX-N4 的 request memory budget 并抽取 CORE-N2；Codex、Antigravity、Claude、Cursor、Grok、Kiro、Qoder、CodeBuddy 八个切片均已完成。
2. 按 Provider 分批实施 CORE-N1；八类 Provider 已全部完成。
3. 实施 EVID-N1 append-only reference delta v2。
4. 执行 CUR-N2、QD-N2 的周期复核。
5. 对 state.rs 和 server_sqlite.rs 做纯结构拆分，不重新设计 authority；已由 `7f2c531`、`4237756`、`a0ec1ea` 完成。

Phase 3 的本地退出条件已满足：依赖方向、锁顺序、wire golden、故障测试和存储 audit 全部通过；结论来自行为与审计门禁，而不是文件行数下降。LIVE-N1 仍按独立真实输入继续排队，不因 Phase 3 本地关闭而提升。

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
| `RUST_MIN_STACK=67108864 cargo test --no-fail-fast` | lib 3181 passed/1 ignored；API contract 124 passed；两个独立 integration 各 1 passed；0 failed | 通过 |
| Phase 3 结构专项 | 默认线程栈 `state::tests` 179 passed；SQLite 19 passed；in-flight 关键词 7 passed；Router selection 22 passed | 通过；authority、锁、后台恢复与固定绑定无回归 |
| `scripts/static-checks.sh` | rustfmt、Clippy、JSON/Node/Shell、Provider/产品边界/依赖方向/文档审计全部通过；Provider audit 142 tests、smoke 13 tests、Web 41 files/213 tests 通过 | 通过 |
| 八类 reference delta `--check-sources` | Antigravity、Claude、Codex、Cursor、Grok、Kiro、Qoder、CodeBuddy 均按冻结 Git object 通过 | 通过；不构成 live receipt |
| `scripts/smoke/smoke-local.sh` | health、Web fallback、setup、密码/API Token 登录、Provider/Share 创建通过 | 通过 |
| `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` | `failures=0`，但因主动跳过内置测试产生 1 个 internal blocker；缺 8 个真实环境输入且 deployment 未测，共 9 个 blocker | `decision=blocked` / `blocked_inputs`，符合预期 |
| `git diff --check`、docs index | 无 whitespace 错误；文档索引完整 | 通过 |
| Antigravity 后续专项 | 67 个 Rust 关键词测试通过；13 个 Node 验收/环境门禁测试通过；13 个 source delta、17 条 v2 observation audit 通过 | CORE-N2 `fixture_verified`；两条 rail 仍为 `live_pending` |
| Claude 后续专项 | 249 个 Rust Claude 关键词测试通过；12 个 Node receipt gate 测试通过；13 个 source delta、15 条 v2 observation 默认及 `--check-sources` audit 通过 | CORE-N2 `fixture_verified`；四个 operation 仍为 `live_pending` |
| Codex 后续专项 | 399 个 Rust Codex 关键词测试及新增 steering/reported-model 专项通过；12 个 Node receipt gate 与 7 个付费探针安全测试通过；18 个 source delta、22 条 v2 observation 默认及 `--check-sources` audit 通过 | CX-N5～CX-N12 为 `fixture_verified`；五个真实 operation 仍为 `live_pending` |
| Cursor 后续专项 | 304 个 Rust Cursor 关键词测试、16 个 request-memory 关键词测试通过；Clippy `-D warnings` 通过；7 个顶层 Node receipt gate/21 个断言保持通过；10 个 legacy 字段摘要、1 个 dirty-worktree-excluded snapshot、8 条 v2 observation 默认及 `--check-sources` audit 通过 | CORE-N2 `fixture_verified`；OAuth/API-key 两条 rail 仍为 `live_pending` |
| Grok 后续专项 | 184 个 Rust Grok 关键词测试通过；6 个顶层 Node receipt gate/11 个断言通过；10 个 legacy 字段摘要、2 个 committed-object snapshot、11 条 v2 observation 的默认与 `--check-sources` audit 均通过 | CORE-N2 `fixture_verified`；inference/media/remote_compaction 三个 operation 仍为 `live_pending` |
| Kiro 后续专项 | 102 个 Rust Kiro 关键词测试、21 个 request-memory 测试、14 个 Amazon Q 测试通过；5 个顶层 Node receipt gate 覆盖八个 scope；10 个 legacy 字段摘要、1 个干净 committed-object snapshot、14 条 v2 observation、`CORE-N2-KIRO` 与 3 条 reject 边界的默认及 `--check-sources` audit 均通过 | CORE-N2 `fixture_verified`；八个 auth-kind × region receipt、remote compaction 与 shared cache 仍为 `live_pending`/disabled |
| Qoder 后续专项 | 74 个 Rust Qoder 关键词测试、24 个 request-memory 关键词测试通过；9 个 Node oracle mutation 与 7 个三 rail loopback harness fixture 通过；11 条 v2 observation、`CORE-N2-QODER`、默认审计及 TokenRouter committed-object `--check-sources` 通过 | CORE-N2 `fixture_verified`；Global OAuth、Global PAT、CN OAuth 仍各自为 `live_pending` |
| CodeBuddy 后续专项 | 66 个 Rust CodeBuddy 关键词测试、26 个 request-memory 与 15 个 memory-exhaustion 关键词测试通过；6 个双站 receipt fixture 与 11 个环境门禁测试保持通过；11 个 legacy 字段摘要、1 个 dirty-worktree-excluded snapshot、15 条 v2 observation、`CORE-N2-CODEBUDDY` 与 3 条 reject 边界的默认及 `--check-sources` audit 均通过 | CORE-N2 `fixture_verified`；Intl/CN 两站 receipt 与 CB-N4 仍为 `live_pending`/disabled |

正式全套测试使用仓库 release gate 规定的 64 MiB `RUST_MIN_STACK`；Phase 3 结构专项另在默认测试线程栈运行。离线 readiness 中的 `local-contracts-unverified` 仅表示该命令显式设置了 `RUN_TESTS=0`，不能覆盖上表已独立完成的全量测试；同样也不能消除真实凭据与部署 blocker。本轮按约束没有运行 UI 自动化或部署测试，也没有任何 rail 因 fixture、参考项目结果或本地 smoke 被提升为 `live_verified`。

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

- grok2api@906b9493b099d：reasoning cache 的 4096 entry、30 分钟 TTL、scope key 与 LRU 淘汰只作为 CORE-N2 retained-state 差分信号；它不证明单 proof 字节上限或统一 request budget。
- grok2api@7f3f3d3c：visible/plaintext-ratio 质量重试，仅作不采纳评估。
- sub2api@ab99d56e9626e 提交态；工作树修改不作证据。

Kiro：

- kiro.rs 3219d1c、db3e912：region 与 context_window，目标已有等价实现。
- kiro.rs f413e7d、3194bb2：裸 namespaced tool 与 profileArn fallback。
- kiro.rs 0b8c7de、d62054f、13763b6：remote compaction。
- kiro.rs@be0c042：16 MiB EventStream frame 与 rolling-buffer ceiling 只作为 CORE-N2 retained-state 差分信号；不证明统一 request budget、图片膨胀或下游 transform。

Qoder：

- TokenRouter@7faf9469bc695 的 qoder_gateway_handler.go 增量；无专项 wire 变化。
- 同一 committed object 的 qoder/client.go、qoder_gateway_service.go 与 gateway_service.go：4 MiB 控制面响应读取、500 MiB 默认 SSE line ceiling 和非流事件数组只作为 CORE-N2 retained-state 差分信号，不证明统一预算。
- 本仓库 assets/contract/qoder-cli-oracle.json 继续是高于兼容参考的第一来源。

CodeBuddy：

- cli2api cdc80d6：中断工具轮次/11148。
- cli2api a9ae393：namespace/reasoning。
- cli2api 6ac6f83、d361559、eef5b2c：deepseek-v4.1-flash。
- cli2api 76c4dab：stream cancellation。
- cli2api@624874a 的 workbuddy/client.go 与 sse.go：1/16 MiB body 读取上限、16 MiB SSE 单行 ceiling 和非流 content/reasoning/tool-call retained aggregation 只作为 CORE-N2 差分信号，不证明统一 sticky 生命周期预算、payload/header 副本、下游 transform、容量分类或 401 恢复语义。

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

当前判定：第 1～7、9～12 项已在本地范围内满足；第 8 项仍按 LIVE-N1 保持 `live_pending`，因为没有提供真实凭据、Router/Share 环境和部署输入。八类 Provider 已全部完成 CORE-N1、CORE-N2 与 EVID-N1，Phase 3 的 state/SQLite 纯结构拆分也已通过完整本地门禁。因此可以关闭首轮静态差分、P0/P1 离线实施、八类 Provider lifecycle/request-memory 拆分、证据迁移及本地架构切片；完整计划仍不能标记为全部完成，唯一剩余类别是必须由真实环境输入逐 rail 关闭的 LIVE-N1。

## 17. 第二轮增量审计（2026-09-28）

本节是对第 0～16 节的追加审计，不回写、覆盖或重新解释首轮已经冻结的实现状态。第二轮目标基线为 `cc-switch-server@6b9b838`；外部参考只读取 Git 提交对象，不把外部工作树、测试目录或运行时代码引入本仓库。首轮的 `fixture_verified`、`live_pending` 和 reject 结论全部继续有效。除明确带“实施更新”的项目外，下文新增 N 编号仍处于计划阶段，不能倒推为已经实现。

### 17.1 参考冻结点与审计完整性

| 类别 | 推荐参考 | 第二轮冻结 HEAD | 工作树与读取方式 | 相对首轮结论 |
| --- | --- | --- | --- | --- |
| Antigravity | CLIProxyAPI、Antigravity-Manager | `acdace936fa7d`、`0269f045f4e35` | 均按 committed object 读取 | CLIProxyAPI 有新的请求转换缺陷修复；Manager 的签名位置和 daily endpoint 差异仍需真实差分 |
| Claude | CLIProxyAPI | `acdace936fa7d` | committed object | 新增相邻文本/citation 与非法工具名处理证据 |
| Codex | CLIProxyAPI、codex2api | `acdace936fa7d`、`9368ed82e039` | committed object | 新增 schema、nested cache hint、私有事件、reasoning、metadata、model 观测和 WS 证据 |
| Cursor | OmniRoute | `443d66996d69` | 工作树有本地修改；全部排除，只读取 HEAD | 提交态只有依赖/流时序工具变化，没有 Cursor Provider wire 增量 |
| Grok | grok2api、sub2api | `5e5ad75556b6`、`ab99d56e9626` | grok2api committed object；sub2api 工作树修改全部排除 | grok2api 新增 Build 1.0.40 与 catalog-driven capability；sub2api 无新提交 |
| Kiro | kiro.rs | `be0c04219d9d` | committed object | 与首轮冻结点相同，无新增提交 |
| Qoder | TokenRouter | `6d676f1d100e` | 当前 HEAD 的 AGENTS 要求使用 `project-doc` skill；本环境不可用，且工作树非净，因此未读取该 HEAD 的业务文件 | 保留首轮 `7faf9469bc695` 冻结证据；第二轮最新 HEAD 明确标记 `unreviewed_skill_gate`，不能写成 no-wire |
| CodeBuddy | cli2api | `34c91b899aa2` | 两个未跟踪文件 `proxy.html`、`proxy.md` 排除；只读取 committed object | 新增 cache usage、incomplete terminal、malformed arguments、namespace/custom-tool 证据 |

这里的“无新增 wire”只表示两个冻结点之间没有发现 Provider 请求/响应协议行为变化，不表示真实账号已验收。Qoder 的限制也不是推测性“无变化”：最新 HEAD 尚未完成合规读取，必须在 `project-doc` 可用后单独补审。

### 17.2 第二轮执行结论

第二轮共确认 15 个静态缺口；另有 3 个正式编号的先差分或真实验收项（CX-N11、CX-N12、GR-N4），以及 1 组未编号的 Antigravity Manager 差异对比。最紧急的不是增加新 Provider，而是防止 reasoning/citation/tool 语义丢失、未知 tool choice 权限放宽、Codex 私有事件泄漏以及 CodeBuddy cache usage 丢账。

| 优先级 | 新项目 | 当前状态 | 主要风险 |
| --- | --- | --- | --- |
| P0 | AG-N7～N11 | `fixture_verified`（`aeeaf1a`） | reasoning 可见性、工具结果邻接/`$ref`、tool choice 与 ID 映射已在本地闭环；两条 live rail 未提升 |
| P0/P1 | CL-N5、CL-N6 | `fixture_verified`（`7794033`） | 相邻 text/citation 与非法工具名可逆 alias 已闭环；真实 Claude OAuth operation 未提升 |
| P0 | CX-N5～N7、CX-N9 | `fixture_verified`（`f890774`） | schema/item 精确清理与私有事件闭集已闭环；真实 rail 未提升 |
| P0 | CB-N6 | `planned_confirmed_gap` | cache usage 丢失 |
| P1 | CX-N8、CX-N10 | `fixture_verified`（`f890774`） | 真实 reasoning 复用与本地 reported-model 诊断已闭环；不改变路由/计费 |
| P1 | GR-N3 | `fixture_verified`（`a0f567a`） | exact-scope 目录能力与 HTTP/WS 运行时闭环已完成；真实 entitlement 未提升 |
| P1 | CX-N11、CX-N12 | `fixture_verified_after_red_differential`（`f890774`） | 红灯后完成有界 steering writer 与 strict response schema 窄修复 |
| P1 | GR-N4 | `live_pending_no_wire_change` | Grok 客户身份/header 缺固定账号真实证据 |
| 复核 | Cursor、Kiro、sub2api | `reviewed_no_wire_delta` | 不制造无意义生产改动，只刷新冻结证据 |
| 受阻 | Qoder 最新 HEAD | `unreviewed_skill_gate` | 缺参考仓库要求的 skill；不得用猜测补结论 |

音频、视频、文件输入、Gemini Interactions、客户端指纹/attestation、商业定价、账号池和跨身份 fallback 均不因参考项目出现而进入本轮实现范围。

### 17.3 Antigravity 增量差异

目标已有 grounding/citation、模型能力快照、固定 Account/rail、reasoning replay 和完整的请求内存边界，但第二轮参考修复暴露了五个更窄的转换问题。

| ID | 参考证据 | 目标差异 | 计划与验收 |
| --- | --- | --- | --- |
| AG-N7 | CLIProxyAPI `6dea3dfa` 的 `enableAntigravityResponsesThinkingSummary` | Responses 的 `reasoning.summary` / `generate_summary` 没有穿过 Responses→Anthropic→Gemini 链；只要 effort 开启，目标可能仍发 `includeThoughts=true`，显式 `summary:none/null` 也无法关闭可见 thought | 冻结 effort-only、auto/detailed、none、null、无 reasoning 五组 fixture；把“是否推理”和“是否展示 summary”分开解析。显式关闭必须得到 `includeThoughts=false`，未指定且 effort 开启时才采用受证默认；不得把 summary 文本写日志 |
| AG-N8 | CLIProxyAPI `3de5709d` 的 `SplitGeminiFunctionResponseTurns` | 混合 `tool_result + text/reminder/media` 当前会落在同一个 Gemini user content 中，不能保证 functionResponse 紧邻产生它的 model functionCall | 只在紧随含 functionCall 的 model turn 的连续 user run 内拆分和重排：先合并匹配的 functionResponse，再保留原 text/reminder 顺序；覆盖并行结果、跨多条 user message、thinking signature、错配/孤立结果，绝不伪造 pairing |
| AG-N9 | CLIProxyAPI `5af6cd75` 的 `ContainsJSONRef` / `SetGeminiFunctionResponseResult` | function response 内递归出现字符串 `$ref` 时仍作为结构化对象发送，Gemini/Vertex 可能把它解释为媒体引用并以 400 拒绝 | 仅在 Gemini functionResponse 的 result/response 边界递归检测 `$ref`，命中后把完整结果作为 opaque JSON 字符串置于 `response.result`；普通用户 JSON、tool arguments、schema declaration 不受影响，并受既有深度/字节预算约束 |
| AG-N10 | CLIProxyAPI `49eec664` 的 fail-closed tool-choice mapping | 未知字符串/对象类型或缺失名称会被省略，等价于把调用方限制静默放宽为 auto | 未知类型、空 named choice、清洗后冲突必须稳定 400；若兼容路径不能返回错误，最宽也只能显式 `none`，不得变成 auto。`none/auto/required/named` 与 parallel 标志分别做 mutation fixture |
| AG-N11 | CLIProxyAPI `580df95a` 的请求级 ID 清洗和 collision map | Antigravity 的 call/result ID 没有完整的请求级 raw↔wire canonical map；非法字符清洗后可能碰撞，`call573` 与 `call_573` 兼容形态也缺少受控 lookup | 建立仅存活于单请求的映射：合法 ID 原样保留，非法 ID 稳定清洗，碰撞加确定性后缀，response 始终按 raw ID 取回结果；兼容 lookup 只在唯一候选时生效，歧义 fail closed。禁止全局缓存或跨请求复用 |

AG-N7～N11 都先以目标当前输出生成红灯 fixture，再做最小 Provider/转换层修复。Antigravity-Manager 的签名字段位置、daily quota endpoint 和请求身份差异仍列为 differential/live gate；没有两条 OAuth rail 各自的新鲜 receipt 时，不改变现有 rail 或 capability 状态。参考中的音频、视频、文件和 Interactions 支持也不直接采纳，因为本产品尚无对应的 Provider 合同和真实验收范围。

实施更新（2026-09-28）：AG-N7～N11 已在 `aeeaf1a61e20d8e55c0569c67894fb31dfd54feb` 完成。新增 Provider-local 最终 wire 归一化，覆盖 Responses summary 可见性、混合 tool result 邻接、递归 `$ref` opaque 编码、source/native Gemini tool choice 拒绝、工具名碰撞以及请求级 raw↔wire tool-call ID 映射；reasoning replay 补回签名调用后会再次执行同一归一化。Antigravity 关键词测试 69/69、全量 Rust（64 MiB 正式测试栈）、Clippy、reference-delta 默认/`--check-sources` 与静态 Provider/coverage/docs 审计均通过。证据追加为 `AG-OBS-0018`～`AG-OBS-0022`，五项状态均为 `fixture_verified`；两条 OAuth rail、AG-N6 与 compaction 继续 `live_pending`/disabled。

### 17.4 Claude 增量差异

| ID | 参考证据 | 目标差异 | 计划与验收 |
| --- | --- | --- | --- |
| CL-N5 | CLIProxyAPI `781a203b` | Anthropic→Responses 的流式与非流式转换会把相邻 text block 当成独立 content part；每个 block 调用 citation 映射时 base 都从 0 开始，合并后的文本与 annotation offset 不一致 | 相邻 text block 共用一个 assistant message/output item，并以累计 Unicode scalar 长度推进 citation base；thinking、tool、server tool 或 refusal 才切断合并。覆盖跨 chunk、重复 cited text、组合字符、emoji、无 citation 和中途 tool block |
| CL-N6 | CLIProxyAPI `75b854eb`；目标 `normalize_claude_oauth_tool_names` | 目标已具备稳定 alias 和响应恢复，但只有 `CC_SWITCH_CLAUDE_CUSTOM_TOOL_ALIAS=1` 才为普通自定义工具启用；默认路径仍可能把不符合 `^[A-Za-z0-9_-]{1,64}$` 的名称发往 Claude OAuth | 对“确实非法”的自定义工具名默认自动 alias；已经合法的名称逐字节保留，内建/server tool 不改。声明、tool_choice、历史 tool_use/tool_reference、流式和非流式响应共用请求级 map；case-insensitive/截断碰撞 fail closed。保留 incident rollback，但不再要求正常用户主动开 flag |

CL-N5 只修改 Responses 输出组织与 offset，不把不同语义 block 粘在一起。CL-N6 不迁入参考项目的 credential cloaking、浏览器伪装或指纹逻辑；Opus 5.5 / Claude Code 2.1.280 已由既有提交覆盖，本轮不得重复实施。

实施更新（2026-09-28）：CL-N5～CL-N6 已在 `77940334fda6291706d3807d0d3cdcb49c767d65` 完成。流式和非流式 Anthropic→Responses 现在按语义边界聚合相邻 text block，citation 以 Unicode scalar 累计且每个新 block 从其自身文本起点搜索；重复 cited text、组合字符、emoji、无 citation 与中途 tool/reasoning 边界均有 fixture。Claude OAuth 默认只 alias 真正非法的普通 custom/MCP 名称，合法名称逐字节保留，server tool 不改；declaration、choice、history、JSON/SSE 回映共享请求级 map，大小写和 wire alias 碰撞 fail closed，并保留 `disabled` 事故回滚。Claude 关键词测试 270/270、Anthropic 关键词测试 131/131、Clippy、wire profile、reference-delta 默认/`--check-sources` 审计均通过；证据追加为 `CL-OBS-0016`～`CL-OBS-0017`，两项均为 `fixture_verified`，五个真实 operation 与非法 MCP live acceptance 仍为 `live_pending`。

### 17.5 Codex 增量差异

| ID | 参考证据 | 目标差异 | 计划与验收 |
| --- | --- | --- | --- |
| CX-N5 | CLIProxyAPI `320100ec` | `has_incompatible_codex_unicode_escape` 只识别活动 `\p{}` / `\P{}`，不识别严格校验器拒绝的 octal NUL `\0` | 在 schema-aware 节点内识别活动 `\0`（含奇偶反斜线和 JSON 解码后的形态）并只删除该 `pattern`；合法 `\x00`、普通 regex 及 default/enum/description 内同名字段保持，归一化须幂等 |
| CX-N6 | CLIProxyAPI `3662d153` | Codex OAuth sanitizer 没有移除 `prompt_cache_breakpoint`；item 顶层、message `content[]` 和 function output `output[]` 都会原样发网 | 只在 `input[*]` 顶层及其协议 content/output part 删除精确字段；字符串 output、文本、顺序、arguments 内同名 JSON 和其他嵌套用户数据原样保留 |
| CX-N7 | CLIProxyAPI `dd013f9e` | Responses 同格式流存在原样/数据帧透传分支，未知 `responsesapi.*`、`codex.rate_limits` 或私有 `codex.*` 事件可能下泄 | 在统一 SSE framing 边界同时检查 declared event 与 payload `type`：总是过滤 `responsesapi.*` 和 `codex.rate_limits`；普通客户端只允许公开 Responses 闭集，明确识别的原生 Codex 客户端也只放行必要 metadata allowlist；error/terminal 不得误吞 |
| CX-N8 | CLIProxyAPI `40cc6489` | Responses→Chat 只把真实 reasoning 附到第一个工具轮；同一逻辑 turn 的后续连续 tool call 没有新 reasoning 时不会复用最近一次真实 reasoning | 请求内维护“最近真实 reasoning”，连续工具轮复用，遇到新 reasoning 更新，遇到 user/system 边界清空；无历史 reasoning 时保持缺省，明确禁止参考实现中的 `[reasoning unavailable]` 伪造文本 |
| CX-N9 | CLIProxyAPI `3b2882b7` | `sanitize_codex_oauth_request_body` 目前只删 item 顶层 `internal_chat_message_metadata_passthrough`，未删同层 `author`、`recipient` | 扩展为精确 item-level 三字段清理；content 内普通同名键、arguments 字符串/对象和顶层非 input 数据不删。覆盖 Responses、Compact、WS→HTTP fallback，并保持幂等 |
| CX-N10 | CLIProxyAPI `25f40d8c`、codex2api `dbfd3f89` | usage 记录的是选择后/实际发送的 `actual_model`，没有从 JSON 响应或 stream terminal 观察上游真实 model，静默换模不可见 | 增加有界 response-model observer，分别记录 requested/sent/reported model 与来源；仅告警和审计，不改变响应、计费或路由，不触发 retry/fallback；缺字段保持 unknown，不猜测 |
| CX-N11 | CLIProxyAPI `42c9680e` | 目标 WS 已能转发 steering，但当前一次 upstream write 未完成时会拒绝新的 downstream data；是否在真实 Codex 并发上传中造成拒绝尚无本地证据 | `differential_first`：用受控 WS fixture 并发发送碎片化 tool input、steering、ping/pong 与 cancel，量化当前拒绝/延迟；只有红灯才拆成全双工 reader/writer actor，队列继续有界且 control frame 优先 |
| CX-N12 | codex2api `288a28cb` | 工具 schema 有保守归一化，但 `/text/format/schema` 未走等价处理；strict response schema 中“有 required、无本层 properties”的节点可能带 orphan key | `differential_first`：冻结 composition branch、纯 object、`$ref/$dynamicRef` 三形状。只在真实差分红灯后复用 schema-aware 算法：组合分支按可见字段裁剪、无字段来源的 object 删除 required、外部引用保持；禁止递归删除未知用户字段 |

以下 Codex 变化不形成新实现项：service tier/routing hint 已由现有 policy 覆盖；WebSocket upstream relay 和 SSE event boundary 已有等价保护。WS prewarm 仍是独立 `live_pending`，第二轮静态审计不能把它提升为已支持。codex2api `e15cd28d` 删除 client harness scaffolding 会重写用户文本，标记 `not_adopted`；Windows attestation、浏览器/客户端指纹和未获真实证据的身份模拟也不复制。

实施更新（2026-09-28）：CX-N5～CX-N12 已在 `f8907742b163b239a981cddcc3962410f0712677` 完成。schema sanitizer 识别活动 octal NUL 并保持非 schema decoy；Responses、Compact、WS 与 WS→HTTP fallback 共用精确 item/part metadata 清理。SSE 以 declared event + payload type 双重闭集过滤，连续工具轮只复用真实 reasoning。JSON/SSE/WS 的上游自报 model 以有界、终态优先方式写入本地 UsageLog/Web API，不覆盖 `actual_model`，不参与路由、计费、retry/fallback，也不进入 Router Share usage payload。

CX-N11 与 CX-N12 的差分 fixture 均先复现红灯后再窄修复：WS writer 使用容量 8 的有界队列，大写入期间只接收精确顶层 `response.steer`，queue full 与伪装/第二 create/坏帧 fail closed，Ping/Pong 和取消保持响应；两个 text-format schema 入口会裁剪 composition orphan required，纯 object 无字段来源时删除 required，`$ref/$dynamicRef` 保持 opaque。Codex 关键词测试 399 项及新增 steering/reported-model 专项、Clippy、API contract、reference-delta 默认与 `--check-sources` 审计均通过；证据追加为 `CX-OBS-0014`～`CX-OBS-0022`，八项均为 `fixture_verified`。GPT Image 2.5、WS prewarm、Astra entitlement 等五个真实 operation 继续 `receipt=null/live_pending`。

### 17.6 Cursor 增量复核

OmniRoute 从首轮 `02c663cdd0e85` 到 `443d66996d69` 的 committed delta 只涉及依赖和共享 stream-timing 工具，没有发现 Cursor OAuth/API-key 请求头、protobuf/gzip、Agent plan、模型目录、工具桥或 terminal wire 的变化。其工作树内 Codex 相关修改全部排除，不可作为 Cursor 证据。

第二轮状态为 `reviewed_no_wire_delta`：不新增 CUR-N 实现项，不重写现有 executor。CUR-N1 的 OAuth/API-key 两条真实 rail、既有 request-memory 和 parked-session 合同继续按首轮状态验收。

证据更新（2026-09-28）：`assets/contract/cursor-reference-delta.json` 已追加 `omniroute-2026-09-28` snapshot 与 `CUR-N2-2026-09-28` immutable review extension。默认 audit 与 `--check-sources` 均确认 14 个 committed path inventory 精确、六个 Cursor wire path 零变化；22 项工作树内容全部排除。该更新只冻结 `reviewed_no_wire_delta`，没有生产代码提交，也没有提升 OAuth/API-key 两条 `live_pending` rail。

### 17.7 Grok 增量差异

| ID | 参考证据 | 目标差异 | 计划与验收 |
| --- | --- | --- | --- |
| GR-N3 | grok2api `9dda42ae` | `GrokModelCatalog` 只保存 model ID；目录里的 reasoning menu/default、`supports_reasoning_effort`、context window、max completion 和 backend search 被丢弃。运行时 `grok_model_supports_reasoning_effort` 仍是静态前缀表，不含 Grok 4.7、minimal/max 的目录约束 | 新增 `GrokModelCapability`，与现有 `GrokModelCatalogScope` 一起按 App+Provider revision/runtime+Account+auth/token generation 原子替换；reasoning 只接受当前精确 snapshot 公布的有序菜单，explicit zero/empty/unknown 分开处理。stale catalog 可展示但不得扩大运行时权限；成功空目录权威替换。不要复制参考的 process-global profile map |
| GR-N4 | grok2api `9dda42ae` | 参考使用 Build `1.0.40` 并派生 `x-grok-conv-group-id`；目标默认身份仍为 `grok-shell/0.2.111`，只发 `x-grok-conv-id`，但两个版本谱系是否可直接替换没有真实证据 | `differential_first/live_only`：先用同一固定 Account 分别验证 catalog 与 inference 的 accepted identity/header。只有明确 version-rejected signal 或 fresh receipt 才更新 reviewed 常量；group id 若启用，按已 namespace 的 root session 做稳定 UUIDv5，不能暴露原始用户/session，也不能跨 Share/Account 合并 |

GR-N3 的 capability 只影响模型展示、请求校验和受证 upstream control，不把 catalog context window 当成本地内存预算，也不自动开启搜索/compaction。参考项目把 Grok 4.7 暂按 4.6 定价是作者推断，缺 xAI 权威价格，因此明确 `not_adopted`。sub2api 在首轮冻结点之后无提交，工作树内容继续排除。

实施更新（2026-09-28）：GR-N3 已在 `a0f567a60d6285cbd9d1e6b4b8396ee95582be3c` 完成。model ID 与 `GrokModelCapability` 在同一个 exact-scope snapshot 中原子替换；parser 保留已知 effort 的上游顺序并去重，default 必须属于菜单，显式 false/zero 与缺失值分开。成功空目录权威清除旧能力，stale 只可展示；HTTP、WebSocket 和 WS→HTTP fallback 只读取当前 App、Provider ID/revision/runtime、Account、auth/token generation 的 fresh capability。API manifest v2 展示 reasoning/context/max/search，但不自动开启搜索、compaction 或改变本地内存预算，也未复制 process-global profile map。

证据更新追加 `grok2api-2026-09-28` snapshot、两个 committed source delta、`GR-OBS-0012`～`GR-OBS-0013` 和 `GR-N3/GR-N4` evidence extension。Grok 关键词回归 224 项、Clippy、默认 audit 与 `--check-sources` 均通过。GR-N3 达到 `fixture_verified`；GR-N4 明确保持 `live_pending`，继续使用 `0.2.111` 且不发送 `x-grok-conv-group-id`，直到同一固定 Account 的 catalog/inference fresh receipt 或明确 version-rejected signal 齐备。

### 17.8 Kiro 增量复核

kiro.rs 的第二轮 HEAD 仍是 `be0c04219d9d`，与首轮冻结点一致。没有新的 committed wire 可比较，因此状态为 `reviewed_no_wire_delta`。KI-N3 的 remote compaction、auth-kind × region receipt 和 KI-05 shared cache 继续保持 `live_pending`/disabled；“参考仓库没变化”不能替代这些真实验收。

证据更新（2026-09-28）：`assets/contract/kiro-reference-delta.json` 已追加同 commit/tree 的 `kiro-rs-2026-09-28` snapshot 与 `KIRO-NO-WIRE-2026-09-28` immutable review extension。15 个已冻结的 Kiro wire/evidence path 全部重新核对，提交区间、`changedCommittedPaths` 与 `changedKiroWirePaths` 均为空；默认 audit 与 `--check-sources` 均通过。该复核没有生产代码改动，也没有提升八个 auth-kind × region receipt、remote compaction 或 shared cache。

### 17.9 Qoder 审计边界

TokenRouter 最新 HEAD `6d676f1d100e` 的仓库指令要求 `project-doc` skill。本环境没有该 skill，本轮也不能绕过指令直接读取业务代码；其非净工作树更不能充当证据。因此：

- 首轮基于 `TokenRouter@7faf9469bc695` 的 QD-N1/QD-N2、CORE-N1/N2 和三 rail live gate 保持有效；
- 最新 HEAD 只记录 commit identity，不给出“已复核无 wire 变化”的结论；
- 后续在 skill 可用时，从 `7faf9469bc695..6d676f1d100e` 做独立 committed-object 增量审计，再决定是否新增 QD-N 项；
- 在此之前不改变 Global OAuth、Global PAT、CN OAuth 的任何 capability 或 `live_pending` 状态。

### 17.10 CodeBuddy 增量差异

cli2api 的新增提交大部分已被目标共享 bridge 覆盖，但 cache usage 存在一个可复现的端到端缺口。

| ID/结论 | 参考证据 | 目标判定 | 计划与验收 |
| --- | --- | --- | --- |
| CB-N6 | cli2api `a9da609`、`fff3419` | `CodeBuddyChatSseAggregator` 会保存完整 usage，`usage_from_json_with_semantics` 也能给内部日志识别 `cache_read_tokens/cache_write_tokens`；但 `openai_responses_usage_from_chat_usage` 和 `anthropic_usage_from_openai_usage` 不读取这两个顶层字段，因此 Chat→Responses/Messages 的下游 usage 丢失 cache read/write | 在 CodeBuddy canonical SSE/aggregate 边界规范化 usage，同时保留原始字段：read 优先级为显式 `cache_read_tokens` → nested cached → vendor legacy，write 同理；显式 0 不得变 unknown。覆盖 stream/non-stream、Chat/Responses/Messages、top-level-vs-nested 冲突和 total token 不重复相加 |
| 已覆盖 | cli2api `f44e887` | 非流 `openai_chat_response_to_responses` 与流式 `ChatResponsesState::finish_stream` 已把 `finish_reason=length` 转为 `response.incomplete` + `max_output_tokens` | 补 CodeBuddy 专项差分 fixture 即可，不复制 Provider 私有 terminal 状态机 |
| 已覆盖 | cli2api `b415dd0`、`9f9b66e` | 共享 `ResponsesToolContext` 已覆盖 namespace identity、custom tool 声明/history/choice、流与非流响应恢复，并有 collision/长名称测试 | 状态保持 `fixture_verified_existing_shared_bridge`；只补一条 CodeBuddy 端到端 fixture，避免重复分支 |
| 不采纳 | cli2api `aeaa4ac` | 参考对请求历史中的 malformed function call/result 可整对跳过，非流响应还会静默丢弃坏 call；目标对已完成的 malformed arguments 稳定 fail closed | 保持目标更严格策略；stream 必须在任何 done/completed 前失败，非流返回稳定协议错误，不隐藏损坏数据，也不伪造 `{}` |

cli2api 的统一 check-in、套餐到期展示和账号路由属于运营/控制面，不是本仓库固定 Provider/Account 的反代 wire，不进入本轮计划。Intl/CN 两站 receipt 与 CB-N4 仍各自 `live_pending`/disabled；cache fixture 通过不能提升任何站点的 live 状态。

### 17.11 实施批次与依赖

| 批次 | 范围 | 进入条件 | 退出条件 |
| --- | --- | --- | --- |
| R2-0 证据冻结 | 为新 ID 增加 append-only observation、最小脱敏 fixture 和 target/source digest | 本文合入后 | 所有 confirmed gap 在未修前稳定红灯；外部仓库不成为测试依赖 |
| R2-1 安全与拒绝面 | AG-N10、CX-N7、CX-N9、CX-N5、CX-N6 | R2-0 完成 | 未知权限不放宽、私有事件不泄漏、精确 sanitizer mutation tests 全绿 |
| R2-2 语义守恒 | AG-N7～N9、AG-N11、CL-N5、CB-N6 | R2-0 完成，可与 R2-1 分 Provider 并行 | reasoning、citation、tool ID/result、cache usage 在流/非流和三 Surface 守恒 |
| R2-3 兼容与观测 | CL-N6、CX-N8、CX-N10、GR-N3 | 对应失败 fixture 已冻结 | 合法输入不改写；新增 alias/reasoning/model/catalog 行为均有作用域、代际和内存边界 |
| R2-4 差分/真实验收 | CX-N11、CX-N12、GR-N4、Antigravity Manager 差异、既有 LIVE-N1 | 有目标 fixture 或真实凭据 | 差分绿则只记证据；差分红才做窄修复；live receipt 按 rail/site/operation 独立判定 |
| R2-Q Qoder 补审 | TokenRouter 最新增量 | `project-doc` skill 可用且仅读 committed object | 形成明确 `reviewed_no_wire_delta` 或新增 QD-N，不允许长期用“未知”冒充完成 |

每个批次按 Provider 小提交落地，不能顺便重构 4 万行 forwarder。共享 helper 只有在至少两个 Provider 的合同完全相同且 mutation fixture 能证明边界时才抽取；否则保留 Provider-local policy。

### 17.12 新增验收矩阵

| 范围 | 离线必须证明 | 仍需真实输入的部分 |
| --- | --- | --- |
| AG-N7 | summary none/null 不展示 thought；auto/default 映射稳定；effort 不被误关 | 不提升 Antigravity 两条 rail |
| AG-N8/N9/N11 | 混合 turn 邻接、并行/冲突 ID、`$ref` opaque result、无跨请求状态 | 厂商接受性异常才补 receipt，不以 fixture 宣称 live |
| AG-N10 | unknown/empty/collision 不会变 auto；合法 choice 不回归 | 无 |
| CL-N5 | Unicode scalar citation offset、相邻 block 合并、流/非流事件顺序一致 | Claude OAuth operation 仍按既有 receipt gate |
| CL-N6 | 只有非法名称 alias；声明/history/choice/response 可逆；碰撞 fail closed | 至少一个真实非法 MCP 名称 receipt 才能移除 incident gate |
| CX-N5/N6/N9 | schema 与 item-level 精确 mutation；arguments/用户内容 decoy 不变 | 无 |
| CX-N7 | declared event 与 payload type 双重过滤；error/terminal/公开事件保留 | 原生 Codex metadata allowlist 如有变化需真实客户端 receipt |
| CX-N8 | 连续 tool turn 复用真实 reasoning，用户边界清空，不伪造 placeholder | compat model 的真实接受性按现有 operation gate |
| CX-N10 | JSON/SSE/WS reported model 只做审计；缺失/冲突/超长输入有界 | 告警阈值可用线上脱敏样本调优，但不阻塞离线正确性 |
| CX-N11/N12 | 差分红灯已冻结；容量 8 steering 队列、control/cancel 公平性、composition/object/reference 三类 schema 窄修复全绿 | 生产 WS 高并发与厂商 strict schema 接受性仍需真实输入，不因 fixture 提升 |
| GR-N3 | exact-scope catalog replace、generation drift、菜单/default/minimal/max、stale 不扩权 | 搜索、4.7 和 context/max 接受性分别验收 |
| GR-N4 | UUID derivation 和 header 隔离可离线验证 | client version 与 group header 必须有 fixed-account fresh receipt |
| CB-N6 | explicit zero、字段优先级、total 不双计、三 Surface 流/非流一致 | Intl/CN live 状态不变 |

所有 fixture 成功只允许状态到 `fixture_verified`。任何 rail/site/operation 没有 fresh、脱敏、scope 完整的 receipt 时，仍为 `live_pending`；一个站点或 rail 的成功不能外推到另一个。

### 17.13 第二轮不可破坏边界

- Provider、Account、rail、site、Share 和 auth identity generation 继续固定；本轮任何 retry、catalog、alias 或 WS 改动都不得引入池、轮换或跨身份 fallback。
- Sanitizer 必须按协议位置和字段名精确命中，不能递归清理整棵用户 JSON；arguments、prompt、tool output 原文不得进入日志和指标。
- Catalog capability 是精确 scope 的快照，不是进程全局 truth；成功空结果权威，stale 只在既有合同允许的展示场景使用。
- 外部代码只作为研究证据。实现、fixture、CI、构建、发布和运行时都必须自包含于本仓库。
- 能力变化需同步合同源、`PROTOCOL_EVIDENCE.md` 和 reference-delta；`docs/provider/coverage.md` 仍是生成文件，禁止手工编辑。
- 本轮文档/静态审计不运行 UI 自动化或部署测试；将来生产代码实现按第 14 节运行风险相称的 Rust、Node、smoke 和 readiness 门禁。

### 17.14 第二轮证据索引

外部提交：

- Antigravity：CLIProxyAPI `6dea3dfa`（reasoning summary）、`3de5709d`（tool-result adjacency）、`5af6cd75`（function response `$ref`）、`49eec664`（tool choice fail closed）、`580df95a`（tool ID collision）。
- Claude：CLIProxyAPI `781a203b`（相邻 text/citation）、`75b854eb`（Claude tool name sanitizer）。
- Codex：CLIProxyAPI `320100ec`（octal NUL）、`3662d153`（nested cache breakpoint）、`dd013f9e`（private SSE event）、`40cc6489`（reasoning across tool turns）、`3b2882b7`（author/recipient metadata）、`25f40d8c`（reported model）、`42c9680e`（full duplex WS）、`7b6fafce`（mid-connection prewarm）；codex2api `dbfd3f89`（reported-model cross-check）、`288a28cb`（orphan required）。
- Grok：grok2api `9dda42ae`（Build 1.0.40、conversation group 和 catalog capability）。
- CodeBuddy：cli2api `a9da609`、`fff3419`（cache usage）、`f44e887`（incomplete terminal）、`aeaa4ac`（malformed arguments）、`b415dd0`（namespace identity）、`9f9b66e`（custom tools）。
- Cursor：OmniRoute `443d66996d69`；Kiro：kiro.rs `be0c04219d9d`；sub2api：`ab99d56e9626`，均按上述 no-wire 边界解释。
- Qoder：最新 TokenRouter `6d676f1d100e` 仅记录身份，业务内容未审；可依赖的最近冻结对象仍是 `7faf9469bc695`。

目标代码锚点：

| 主题 | 目标路径/符号 |
| --- | --- |
| Antigravity reasoning/tool turn/function response | `src/proxy/transforms.rs` 的 reasoning→Gemini、`anthropic_message_to_gemini_content`、Gemini history ID helpers |
| Claude text/citation | `anthropic_response_to_openai_responses_with_tool_context`、`anthropic_citations_to_responses_annotations`、stream `AnthropicResponsesState` |
| Claude tool alias | `src/proxy/claude_oauth.rs` 的 `normalize_claude_oauth_tool_names` / response restore patcher |
| Codex schema/sanitizer | `src/proxy/tool_schema.rs`、`src/proxy/forwarder.rs` 的 `sanitize_codex_oauth_request_body` |
| Codex stream/WS/model observation | `src/proxy/responses_transport.rs`、`stream_transforms.rs`、`forwarder.rs` 的 Codex HTTP/WS lifecycle |
| Grok catalog/runtime | `src/clients/oauth/grok_models.rs`、`src/proxy/grok.rs`、`src/domain/grok_cli.rs` |
| CodeBuddy usage | `src/proxy/codebuddy.rs`、`openai_responses_usage_from_chat_usage`、`anthropic_usage_from_openai_usage`、`usage_from_json_with_semantics` |

### 17.15 第二轮完成定义

1. AG-N7～N11、CL-N5、CX-N5～N7、CX-N9、CB-N6 均先有失败 fixture，再有最小修复和 decoy/mutation 回归；不能只按参考代码形状重写。
2. CL-N6、CX-N8、CX-N10、GR-N3 的作用域、碰撞、代际、内存和取消边界均有专项测试；合法既有请求逐字节或语义等价。
3. CX-N11、CX-N12、GR-N4 必须留下明确的绿灯/红灯/`live_pending` 判定；静态证据不足时保持不变。
4. Cursor、Kiro、sub2api 保持 `reviewed_no_wire_delta`，不产生伪工作；Qoder 最新 HEAD 在 skill 可用前保持 `unreviewed_skill_gate`，不得假装完成。
5. 新增 usage/model/catalog 观测不得记录 secret、prompt、tool arguments、reasoning 或原始身份，也不得改变固定路由和计费权威。
6. 所有实现仍遵守 pre-commit 恢复、同 Provider/Account/rail/site、总 attempt/time/memory budget 和 post-commit 禁止透明 replay。
7. reference-delta、合同源、`PROTOCOL_EVIDENCE.md`、registry/UI matrix 与生成 coverage 一致，外部仓库未进入依赖或日常 CI。
8. 缺真实凭据的 LIVE-N1、AG-N6、WS prewarm、Grok GR-N4、Kiro compaction/cache、Qoder 三 rail、CodeBuddy 双站继续诚实保持 pending/disabled。

第二轮当前判定：除 Qoder 最新 HEAD 因仓库要求的 skill 不可用而明确受阻外，七类参考和 Qoder 既有冻结证据的 committed-object 增量分析已完成；Antigravity AG-N7～N11、Claude CL-N5～N6、Codex CX-N5～N12 与 Grok GR-N3 已实施并达到 `fixture_verified`，Cursor/Kiro 已完成 no-wire 复核，Grok GR-N4 保持 `live_pending_no_wire_change`，CodeBuddy 仍按表中状态推进。任何 `planned_*`、fixture 或参考项目行为都不能解释为真实账号验收。
