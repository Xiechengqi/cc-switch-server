# cc-switch-server 反代增强增量分析与实施计划

> 文档性质：八类反代的增量差异分析与实施路线图，不是架构或协议真值。架构以 docs/architecture/overview.md 为准，Provider 身份与能力以 assets/contract/provider-registry.json 为准，wire 证据以 PROTOCOL_EVIDENCE.md、厂商材料和本仓库冻结 fixture 为准。
>
> **当前实施结论（2026-10-27）：[§20.11：当前主仓完整集成与最终 R4 review](#enhance-r4-final-review)。** 八类离线增强、CORE-03 跨类统一计时及当前主仓完整后端集成已完成：Rust 4028 通过／2 忽略、后端 Node 1921 通过、本地 smoke 10/10。真实回执仍 `live_pending`，离线 readiness 实际为 `not-ready`；工作树未提交。逐类实施与真实边界见第 20 节，不做 UI 自动化或部署测试。
>
> **当前分析入口（2026-10-27）：[第 19 节：八类推荐反代的源码复核与 R4 增强计划](#enhance-r4-20261027)。** 本轮以目标 `e5b9e83ad488448473ad604fb855fbe28dd5f456` 和各参考仓本地可达 HEAD 为准，已逐类按目录 `AGENTS.md` 研究。R4 项目的当前实施状态见第 20 节；第 19 节的待实施判定属于研究交付时点，第 0～18 节保留为历史记录，其中的旧待办、测试成绩和门禁判定不能直接当作当前状态。TokenRouter 根指令现已恢复可读，其 `project-doc` 技能已读取并遵循，本轮 Qoder 分析未被技能门禁阻断。
>
> 分析日期：2026-09-18，实施状态更新至 2026-09-28。分析起点：`origin/main@7c9ef35`；Provider 生产差分起点为 `4712ca063930fea507910c37749595c8d6acc073`。首轮生产行为与协议证据冻结到 `db65188`，验证门禁收口到 `248e7c2`。后续已按 Provider 完成 Antigravity（`e5bfc34`）、Claude（`6b0a0fe`）、Codex（`069f3ef`）、Cursor（`8f72bb9`）、Grok（`269850a`）、Kiro（`1124082`）、Qoder（`2cc8a01`）与 CodeBuddy（`d81056c`）的首批 CORE-N1/LIVE-N1 切片，并完成八类 EVID-N1 迁移；CORE-N2 最后由 Qoder（`3485571`）和 CodeBuddy（`573dc47`）收口。Phase 3 的本地结构项由 SQLite 关注点拆分（`7f2c531`）、state 并发准入拆分（`4237756`）和后台调度拆分（`a0ec1ea`）收口。第二轮已完成 Antigravity AG-N7～N11（`aeeaf1a`）、Claude CL-N5～N6（`7794033`）、Codex CX-N5～N12（`f890774`）、Grok GR-N3（`a0f567a`）和 CodeBuddy CB-N6（最终复核 `4f8c937`）；整体 review 又以 `05d498c` 收紧 Claude alias 回映的 payload opacity，以 `26eb389` 收紧 Codex 匿名 SSE 和组合引用边界。Cursor 与 Kiro 第二轮均冻结为 `reviewed_no_wire_delta`，GR-N4 因缺固定账号证据保持 `live_pending` 且不改 wire。Qoder 最新 HEAD 仍按第 17 节的 `unreviewed_skill_gate` 等待合规补审。
>
> Provider 协议差异定位从 ae7fc88 开始；提交前 main 新增 6799870（备份保留策略）和 4712ca0（tunnel rotation），已复核其提交态差异，不涉及本文八类 Provider 的协议锚点。本文只分析已提交状态；有本地修改的参考仓库只读取 HEAD 提交态。

> 当前状态索引（2026-10-15）：上方 9 月快照及第 0～17 节是各自日期的历史判定，不代表当前待办。八类 Provider 的本轮离线实施与整体 review 见 §18.322～18.334，随后提交态冻结见 §18.335；TokenRouter 当前工作树删除根 `AGENTS.md` 后，Qoder 最新可达源码已补审，旧 `project-doc` 门禁不再限制当前只读研究，但恢复该文件时须重新核对。真实账号验收已明确排除在本 goal 外，各 rail/operation 仍保持 `live_pending`。§18.331～18.334 所记未提交状态是当时的历史判定，不代表 §18.335 的当前提交态。

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
| CodeBuddy | CB-N1/N2/N6 `fixture_verified`；CB-N3/N5 与第二轮 incomplete/namespace/custom-tool 由共享实现覆盖；CORE-N1 Provider facade、CORE-N2、Intl/CN schema-v2 私有 receipt gate 与 reference-delta v2 已完成 | CB-N4 与两站真实 receipt `live_pending`，v4.1 runtime disabled；malformed arguments 宽松丢弃明确不采纳 |

CORE-N2、CORE-N1 与 EVID-N1 均已完成八个 Provider 切片，Phase 3 的 `state.rs` / `server_sqlite.rs` 本地纯结构拆分也已完成。所有缺真实凭据的 LIVE-N1 operation/rail/site 继续保持门禁；本地容量合同或结构验收完成不等于任何真实订阅状态提升。

本 goal 的完成范围是计划内可在本仓库落地的实现、离线验证与最终整体 review；真实账号 receipt 的采集及厂商接受性验证属于独立的后续验收，不作为本 goal 完成条件。`live_pending` 与 runtime disabled 必须如实保留，不能因此放宽能力门禁或宣称真实通过。

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
| P0 | CB-N6 | `fixture_verified`（`4f8c937`） | cache usage 已在双站、三 Surface、流/非流闭环；真实站点状态未提升 |
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

整体 review 追加修复（2026-09-28）：`05d498cf7bbcf1d4e1e67fd1f422c09bf9e8a456` 修复了 response alias 回映递归进入工具参数的问题。`input`、`arguments`、`args` 与 Gemini `functionResponse/function_response` 以下现在保持 opaque；只有协议定义的 call-name 位置可以回映，参数里伪装成 `function.name`、`functionCall.name` 或嵌套 call 的 decoy 保持原值，JSON 与分片 SSE 均有回归。该结论冻结为 `CLAUDE-R2-FINAL-2026-09-28`，不新增能力、不改变固定绑定，也不提升任何真实 operation。

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

整体 review 追加修复（2026-09-28）：`26eb389e5b0c8457f64bb70f60614ff5b1c1a140` 关闭两处保守性缺口。CX-N7 对同时缺少 SSE `event:` 和 payload `type` 的 JSON 数据帧默认过滤，`[DONE]` 与 keepalive 仍由 transport 层处理；CX-N12 在 `allOf/anyOf/oneOf/then/else` 的任意嵌套分支含 `$ref/$dynamicRef` 时保留当前节点 `required`，避免把引用提供的字段误判成 orphan。两项均有聚焦 fixture，冻结为 `CODEX-R2-FINAL-2026-09-28`；五个真实 operation 状态不变。

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

证据更新（2026-09-28）：`assets/contract/qoder-reference-delta.json` 已追加不可变 `QODER-SKILL-GATE-2026-09-28` reference review extension，冻结最新 HEAD/tree、detached/非净工作树状态、被排除的 `UD AGENTS.md` 与缺失的 `project-doc` skill。该记录明确固定 `businessFilesRead=false`、`businessAnalysisPerformed=false` 和 `productionCodeChanged=false`，最近可信 snapshot 仍是 `tokenrouter-2026-09-19`；默认自包含 audit 通过后也只能证明门禁未被篡改，不能得出 no-wire 结论或提升三条真实 rail。本轮依仓库指令不运行会重新访问 TokenRouter 的 `--check-sources`。

### 17.10 CodeBuddy 增量差异

cli2api 的新增提交大部分已被目标共享 bridge 覆盖，但 cache usage 存在一个可复现的端到端缺口。

| ID/结论 | 参考证据 | 目标判定 | 计划与验收 |
| --- | --- | --- | --- |
| CB-N6 | cli2api `a9da609`、`fff3419` | `CodeBuddyChatSseAggregator` 会保存完整 usage，`usage_from_json_with_semantics` 也能给内部日志识别 `cache_read_tokens/cache_write_tokens`；但 `openai_responses_usage_from_chat_usage` 和 `anthropic_usage_from_openai_usage` 不读取这两个顶层字段，因此 Chat→Responses/Messages 的下游 usage 丢失 cache read/write | 在 CodeBuddy canonical SSE/aggregate 边界规范化 usage，同时保留原始字段：read 优先级为显式 `cache_read_tokens` → nested cached → vendor legacy，write 同理；显式 0 不得变 unknown。覆盖 stream/non-stream、Chat/Responses/Messages、top-level-vs-nested 冲突和 total token 不重复相加 |
| 已覆盖 | cli2api `f44e887` | 非流 `openai_chat_response_to_responses` 与流式 `ChatResponsesState::finish_stream` 已把 `finish_reason=length` 转为 `response.incomplete` + `max_output_tokens` | 补 CodeBuddy 专项差分 fixture 即可，不复制 Provider 私有 terminal 状态机 |
| 已覆盖 | cli2api `b415dd0`、`9f9b66e` | 共享 `ResponsesToolContext` 已覆盖 namespace identity、custom tool 声明/history/choice、流与非流响应恢复，并有 collision/长名称测试 | 状态保持 `fixture_verified_existing_shared_bridge`；只补一条 CodeBuddy 端到端 fixture，避免重复分支 |
| 不采纳 | cli2api `aeaa4ac` | 参考对请求历史中的 malformed function call/result 可整对跳过，非流响应还会静默丢弃坏 call；目标对已完成的 malformed arguments 稳定 fail closed | 保持目标更严格策略；stream 必须在任何 done/completed 前失败，非流返回稳定协议错误，不隐藏损坏数据，也不伪造 `{}` |

cli2api 的统一 check-in、套餐到期展示和账号路由属于运营/控制面，不是本仓库固定 Provider/Account 的反代 wire，不进入本轮计划。Intl/CN 两站 receipt 与 CB-N4 仍各自 `live_pending`/disabled；cache fixture 通过不能提升任何站点的 live 状态。

实施更新（2026-09-28）：CB-N6 的最终复核实现在 `4f8c937a7ad19366d01f36bb994cb84959f129c4` 完成。CodeBuddy decoder 与非流 aggregator 在 canonical 边界归一 cache read/write，顶层显式字段优先于 nested 与 vendor legacy，显式零保持权威，非法负数、小数与越界整数不参与计量；Claude、Codex、Gemini 输出和内部 UsageLog 使用同一优先级，inclusive total 不重复计 cache。Chat→Claude stream 保持工具参数增量输出，捕获 finish_reason 后的独立 usage 尾帧，并仅在唯一 `[DONE]` 与 EOF 均验证后提交成功 terminal。Intl/CN × 三 Surface × stream/non-stream 的 loopback fixture 同时覆盖非零冲突与显式零，usage-tail 截断不会先成功后报错；CodeBuddy 聚焦回归 70/70、Clippy 通过。

证据更新追加 `cli2api-2026-09-28` snapshot、五个 committed source delta、`CB-N6` enhancement、`CB-OBS-0016` 与独立 second-round review。后者冻结 incomplete terminal、namespace identity、custom tool 已由共享 bridge 覆盖且没有 Provider 私有分支，并明确拒绝 malformed arguments 的静默整对丢弃。默认 audit 与 `--check-sources` 均通过；Intl/CN 仍分别为 `receipt=null/live_pending`，CB-N4 继续 runtime disabled。

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

第二轮当前判定：除 Qoder 最新 HEAD 因仓库要求的 skill 不可用而明确受阻外，七类参考和 Qoder 既有冻结证据的 committed-object 增量分析已完成；Antigravity AG-N7～N11、Claude CL-N5～N6、Codex CX-N5～N12、Grok GR-N3 与 CodeBuddy CB-N6 已实施并达到 `fixture_verified`，整体 review 发现的 Claude payload opacity、Codex 匿名 SSE 与组合引用保守性缺口也已窄修复并冻结。Cursor/Kiro 已完成 no-wire 复核，Grok GR-N4 保持 `live_pending_no_wire_change`。CodeBuddy 的 incomplete/namespace/custom-tool 复核为共享 bridge 已覆盖，malformed arguments 宽松丢弃明确不采纳；Intl/CN 仍为 `live_pending`。任何 fixture 或参考项目行为都不能解释为真实账号验收。

## 18. 第三轮差分审计与增强计划（2026-10-08）

### 18.1 范围、冻结点与判定口径

本节只规划下一轮增强，不追认生产实现、真实账号通过或部署通过；第 17 节保留为 2026-09-28 的历史结论。本轮目标基线是 `cc-switch-server@ba85f5fe611de0b8c8d3e9b2c320402cac47f071`，tree `583a799ab04fe133a74451e9c9fbfa0f9df82453`，审计开始时工作树干净。只比较参考项目的 Git 已提交对象；OmniRoute、sub2api、cli2api 的未提交内容全部排除。下表区间左端均为第 17 节的参考冻结点，右端为本轮 HEAD；多类别复用同一参考项目，不重复计数。

| 参考项目 / 覆盖类别 | 冻结区间与本轮 tree | 新提交 | 本轮边界 |
| --- | --- | ---: | --- |
| CLIProxyAPI / Antigravity、Claude、Codex | `acdace936fa7df2905500c7f5e0a97d683138dea..0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`；tree `f43e8e15ea31e3d1dd1cf83250d9fdb627c06525` | 148 | 只采协议、目录和终态差异；其调度、跨账号重选不迁移 |
| Antigravity-Manager / Antigravity | `0269f045f4e35..b601f5e7375343343922ccab5e50653bc3450c3b`；tree `9211a067165c376775c98ecf3d522b5562380ffa` | 174 | 研究单请求 wire；账号池、轮换、故障转移排除 |
| codex2api / Codex、Antigravity、Grok | `9368ed82e039ac8458f9c264b2298b47d0cbf833..b1bfe1b4ae591e19c4028f943b784ca65981e192`；tree `ac06270d0a34c656ba7d7f142ff3c9b13dfa2dbb` | 105 | 身份版本、流错误和新模型只作差分信号，不直接改计费或伪装身份 |
| OmniRoute / Cursor | `443d66996d69d7ecf887fd61158404a3cca1b192..443d66996d69d7ecf887fd61158404a3cca1b192`；tree `8d1c312f61d115767c9478416cb039750eaf113a` | 0 | 22 项未提交改动排除；提交态无新增 Cursor wire |
| Grok/grok2api / Grok | `5e5ad75556b61a2c4a8fcf344d83bfe7760f2b42..7c889a960e2638341b4dae9a5c81af0e0f38c87f`；tree `eec71f0920553af1a1cc67daec838399d364ea92` | 1 | 仅 README/图片，无 Grok wire 变化；不误用同名顶层目录 |
| sub2api / Grok | `ab99d56e9626e6cd731592dae8553c9758a0efa2..ab99d56e9626e6cd731592dae8553c9758a0efa2`；tree `eb5691dbf93e303b3fddcf16370c7ae0bbb7190a` | 0 | 六项未提交改动排除；提交态无新增 Grok wire |
| kiro.rs / Kiro | `be0c04219d9d1b93b7fe5c3d7b9e7c9cf0d05863..9066e9c4fe90c7e97d241900a6ce8eb52cc46903`；tree `5d22f040f0e4b0aa775e61547540b3c040293cd8` | 26 | Region、无参工具、usage、Compact 等按目标现有开关分别判定 |
| cli2api / CodeBuddy | `34c91b899aa2289f5ee49618d2d300f3eeaffedb..a6502521e1e509a4257153025c642240c39cd54b`；tree `876656d0780afde26af9b1ab0a87e0ea58d8c752` | 48 | 仅共享 gateway/executor 中实际适用于 CodeBuddy 的变化；未跟踪 `proxy.html`、`proxy.md` 排除 |
| TokenRouter / Qoder | 最近可信已审对象 `7faf9469bc695`；最新 HEAD 身份 `6d676f1d100e` | 未审 | 当前会话缺仓库 `AGENTS.md` 要求的 `project-doc` skill；未读取最新业务文件，不能写“无 wire 变化” |

`confirmed_static_gap` 表示从两侧函数和可构造反例确认的静态差异，**不是**已运行失败 fixture；`differential_first` 表示先用最小 fixture 比较行为，红灯才实施；`already_covered` 不新增生产代码；`live_pending` 不因参考提交或本地 mock 提升。下述所有改动都须保持固定 App/Provider/Account/rail/site/Share、两代身份绑定、统一 attempt/time/memory budget 与 post-commit 禁止透明 replay。

### 18.2 八类逐项对照

| 类别 | 新参考证据与目标代码锚点 | 本轮判定 | 下一步 |
| --- | --- | --- | --- |
| Antigravity | CLIProxyAPI `7e857c37` 的 per-account catalog fencing/singleflight 对 `src/clients/oauth/antigravity_models.rs::model_catalog`、`src/api/providers.rs` 和 `src/state.rs::record_antigravity_model_catalog_observations_for_generation` | **AG-N12 `confirmed_static_gap`**：目标目录键只有 ProviderType、Account ID、auth identity generation；并发旧 token/旧调用晚返回可清掉新 generation 缓存，且能力证据写入只复核 auth generation，未复核 token refresh generation 或同 scope 最新发布序号 | 先冻结 A/B 响应倒序与 token 更新反例，再做 exact-scope 的请求代际/发布 CAS；成功空目录仍权威，stale 不扩权，不引入账号池 |
| Antigravity | CLIProxyAPI `9b9c2bdf`、`17dc1b81`、`40f4df07`、`a4acc9f7`、`4e4dede4`；Antigravity-Manager `f3d74cf2`、`58a4e1a8`、`04198598`、`c5ad0622`、`a690dfaa`；codex2api `6c5092dc`、`74ede1ee`；目标 `src/proxy/antigravity_replay.rs`、`providers/antigravity/{mod,wire}.rs`、`streaming.rs` | CAQS v4 已作 opaque round-trip、签名 replay 有精确 scope/历史 hash/CAS、Gemini SSE 已按字节缓冲多行和 UTF-8，backend error/terminal 与强制工具选择也有局部合同；参考的双 base64 检查、context-hash batch replay、思考预算优先级与多图顺序尚无逐项差分结论 | **AG-D3 `differential_first`**：用双层 CAQS、错 context hash、思考预算/模型后缀、多图、并行 tool/result、terminal 后断连的脱敏 fixture；绿灯只加证据。不得猜造签名、跨身份复用或在已提交响应后重放。Manager 的池/轮换明确 `not_adopted` |
| Claude | CLIProxyAPI `e6f6f26a` 的 `pause_turn`→Responses incomplete；目标 `src/proxy/transforms.rs::anthropic_responses_incomplete_reason` 和 `src/proxy/stream_transforms.rs::AnthropicResponsesState` | **CL-N8 `confirmed_static_gap`**：两条路径只认 refusal/max_tokens/context_window；收到 `pause_turn` 会输出 `response.completed`，把仍需服务端工具迭代的回合误报完成 | 非流与 SSE 都映射到 `status=incomplete` / `response.incomplete`，`incomplete_details=null`（不是 max_output_tokens）；三 Surface 原生 stop reason 不得误改，terminal/usage 仅一次 |
| Claude | CLIProxyAPI `678a9257`、`a2976eb8`、`e7b22260` 的续线程 alias store 与 missing-thread 错误；目标 `src/proxy/claude_oauth.rs::normalize_claude_oauth_tool_names` | **CL-N7 `differential_first`**：目标别名仅来自当次 `tools`；`thread.type=continue` 且不重发工具声明时 alias map 为空，历史非法名称/响应名称可能无法还原。目标当前没有参考的 thread-state store | 先验证上游是否允许目标当前直通 thread 形状与缺 tools 续接；若红灯，做有界、可取消、按 Account/身份代际/Share/session/previous_message_id 作用域的状态，并在状态缺失时明确失败关闭；不以首条用户文本 hash 代替线程凭证 |
| Claude | CLIProxyAPI `14dc4e05`、`39632e52`、`eca1d113`、`26e5efbb`、`cb9a0b5c`、`958eeca7`；目标 `claude_oauth.rs` 的 caller system 迁移、cache normalization、`output_config.effort`、mid-conversation beta | **`already_covered`/协议差分**：第三方 caller system 已迁入消息，原生 leading system hoist、effort 与缓存断点有专项代码/测试；参考的 cloaking 文字、显式 cache 策略和 tool add/remove beta 不能按代码形状照搬 | 仅补 leading-user run、single terminal user、cache-control 四断点和 add/remove 的跨流/非流 fixture；发现真实接受性红灯才窄修 |
| Codex | CLIProxyAPI `5d8bb3d9` 的 native Responses 历史 `function_call.arguments` 空串归一化；目标 `src/proxy/forwarder.rs::sanitize_codex_oauth_request_body`，对照 `transforms.rs::response_function_arguments_object` | **CX-N13 `confirmed_static_gap`**：Responses→Claude 已把空白参数解释为 `{}`，但 Codex OAuth 原生 Responses/Compact/WS body sanitizer 没做此精确修复；空串可直接到严格 upstream | 只对 `input[*].type=function_call` 的 `arguments` 空/全空白字符串写入字符串 `"{}"`；非空非法 JSON、custom tool、arguments 内嵌同名字段保持原样，HTTP/Compact/WS/WS→HTTP fallback 同测 |
| Codex | CLIProxyAPI `71a5f1f6`、`67cb32b6` 的 Codex 客户端→非 Codex executor 已知整数参数字段归一化；目标 `src/proxy/tool_schema.rs`、`forwarder.rs` | **CX-D3 `differential_first`**：目标 Codex schema 修复偏向严格上游 JSON Schema，未见按目标 executor 和官方客户端识别来限定的 `number`→`integer` 修复；不能把全部 number 改窄 | 固定 `exec_command.yield_time_ms`、`write_stdin.session_id`、普通数值字段、namespace/`additional_tools` 及 Codex→Codex 对照；只有非 Codex 目标红灯才做精确字段、身份和 executor 条件修复 |
| Codex | CLIProxyAPI `91c934f6`、`3ebee065`、`16d98881`、`68afd0d1`、`97dd9eec`、`9e71c20d`；codex2api `34d5022b`、`269400d6` | local shell/apply_patch、citation/search、Responses 终态与 Model 身份已有共享处理或现有门禁；通用 OpenAI Chat 缺 finish_reason 的新修复要按入口验证，不能据标题宣称 Codex OAuth 缺口 | **CX-D4 `differential_first`**：构造首帧前断连、Chat EOF 无 finish_reason、空 function args、citation 与 tool round-trip 的同账号 loopback；只在明确失败入口改动，不动态下载/自动模拟客户端版本，也不改已有 reported-model 审计 |
| Cursor | OmniRoute 提交态 `443d66996d69` 未变化；目标 `src/proxy/cursor/` 和第 17.6 节 CUR-N1/N2 | **`reviewed_no_new_committed_wire_delta`**；未提交 Codex 文件不作 Cursor 证据 | 不新增 CUR-N 代码；OAuth/API Key 两 rail、request-memory/parked-session 和真实 receipt 保持原门禁 |
| Grok | Grok/grok2api 仅 README/图片、sub2api 无提交；CLIProxyAPI `b467a83c`、`90654da5`、`34e73c75` 和 codex2api `7d717d7f` 分别采用不同版本更新策略；目标 `src/domain/grok_cli.rs`、`src/proxy/grok.rs` | **GR-N4 继续 `live_pending_no_wire_change`**：目标版本 `0.2.111`、可配置受审 override 和版本门禁诊断已在；参考 1.0.41/1.0.44/动态 npm 查询相互不能替代固定账号接受性 | 同一固定 Account 的 catalog/inference HTTP/WS 记录 fresh、脱敏的 accepted/rejected version/header receipt，覆盖 `x-grok-conv-id` 与候选 group header；无 receipt 不更新默认版本、不自动联网拉最新版、不提升 search/compaction |
| Kiro | kiro.rs `734bd8a` region、`b870d62` compact prefix；目标 `src/proxy/kiro.rs`、`src/proxy/kiro/endpoint.rs`、`src/clients/oauth/kiro_runtime.rs` | profile ARN region 已驱动 inference/discovery，并有跨 IdC region 测试；Kiro/Amazon Q Compact 已发网前 disabled，故参考 compact cache 改动不构成现在的线上缺口 | Region 保持；KI-N3/共享 cache 必须先独立协议差分和真实 receipt，不能因参考项目可用就开启 |
| Kiro | kiro.rs `93459b5` 无参工具、`700fbdb`/`baacf84`/`dbb8d9c` usage；目标 `src/proxy/kiro.rs::{ToolJsonAccumulator,KiroUsageAccumulator}` | **KI-N4/KI-N5 `confirmed_static_gap`**：`finish()` 将无 stop、零字节工具也判为 incomplete；若已有文本，`finish_events()` 仍可能写正常 `message_stop` 而漏工具。`contextUsage` 百分比回退直接作为输入，未扣本轮输出；有 `metricsEvent`/明确 tokenUsage 时则由现有优先级覆盖 | KI-N4 恢复零字节无参工具为 `{}`、多工具稳定顺序，非空半截优先失败且不可先成功终态。KI-N5 只在经证据确认的 context fallback 分支扣本轮输出，先 shadow 对照再改变 usage authority；明确 metrics/缓存零值和负值 clamp 不回归 |
| Qoder | `Qoder/AGENTS.md` 推荐 TokenRouter；其 `AGENTS.md` 要求本会话加载 `project-doc`，本会话技能列表无该项 | **`unreviewed_skill_gate`**：最新业务提交没有访问；不能以 cli2api `353cb00` 的 Qoder COSY 实现冒充 TokenRouter 最新审计 | 等 skill 可用后另开 committed-object 增量审计，按 Global OAuth/PAT/CN OAuth 三 rail 分开判定；此前沿用第 17.9 节的可信冻结证据和三个 `live_pending` |
| CodeBuddy | cli2api `ead3202` 共享 gateway 在终态后断连计成功；其他新提交集中 Qoder/控制面；目标 `src/proxy/codebuddy.rs`、`src/proxy/forwarder.rs` 与 `forwarder/codebuddy_http_tests.rs` | **`differential_first`**：目标共享 lifecycle 有 terminal 后完成与 usage 日志，但 CodeBuddy SSE 要求唯一 `[DONE]` 且 EOF；客户端在 `[DONE]` 后、上游 EOF 前断连的具体日志归类尚无专项对照，不能直接宣称等价 | **CB-D3 证据补强**：Intl/CN × Chat/Responses/Messages 用 loopback 比较 terminal 前后断连；达到目标完整终态才记成功，提前断连记 cancelled，不能误记 Provider 成功。绿灯不改生产逻辑；CB-N4 和双站 live 仍 disabled/pending |

以下是先写失败 fixture 的最小反例，便于将“静态判断”与“实际修复通过”分开。参考路径均来自上表已提交对象，目标路径均来自本轮目标基线；任何真实请求内容只能脱敏后入库。

| ID | 参考实现/测试锚点 | 目标反例与通过条件 |
| --- | --- | --- |
| AG-N12 | CLIProxyAPI `sdk/cliproxy/antigravity_models.go`、`antigravity_models_fencing_test.go::TestAntigravityProbeFencesOlderTokenResult` | 同账号旧 token 请求 A 挂起，新 token 请求 B 先返回；A 后完成不得覆盖 B 或其能力证据。另测 auth generation 改变后的旧调用不得删除新 key，成功空目录必须清旧 entitlement |
| CL-N8 | CLIProxyAPI `internal/translator/claude/openai/responses/claude_openai-responses_response.go::claudeResponsesIncompleteDetails` | `stop_reason=pause_turn` 的 Claude JSON 与 SSE 分别进入 Responses：目标目前给 completed；修后均为 incomplete、nullable details，输出/usage 不丢且无双 terminal |
| CX-N13 | CLIProxyAPI `internal/translator/codex/openai/responses/codex_openai-responses_request.go::normalizeEmptyFunctionCallArguments` | native Codex OAuth `input` 中历史 `function_call.arguments=""` 或空白字符串应变 `"{}"`；非空坏 JSON 原样给上游判错，用户数据和 custom tool 完全不变 |
| KI-N4 | kiro.rs `src/anthropic/stream.rs::ToolJsonAccumulator::finish`、`src/anthropic/handlers.rs` | EventStream 仅发 `toolUseEvent(id,name,input="",stop=false)` 后 `endEvent`：目标 `finish()` 报 `Incomplete`；已有文本时还会漏工具却发正常终态。修后仅零字节恢复 `{}`，混入非空半截优先失败 |
| KI-N5 | kiro.rs `src/anthropic/stream.rs::resolved_usage`、`src/anthropic/handlers.rs::resolve_non_stream_usage` | 例如 context=80、本轮 output=9 且无明确 input usage，目标以 80 作输入再报 9 输出；经 receipt 证实 context 包含本轮输出后应分摊输入 71、输出仍为 9。明确 metrics/tokenUsage 存在时不可扣第二次 |

### 18.3 最小实施批次

| 顺序 | 范围 / 优先级 | 进入条件与明确验收 |
| --- | --- | --- |
| R3-0 证据冻结 | AG-N12、CL-N8、CX-N13、KI-N4/KI-N5 | 先在本仓库加入脱敏的 red fixture、源/目标 commit/tree/path/symbol 与预期失败结果；现有 v2 reference-delta 只 append observation，不改写旧记录。Qoder 不运行 `--check-sources` |
| R3-1 终态与工具正确性 | CL-N8、CX-N13、KI-N4，P0 | stream/non-stream、三 Surface 和 HTTP/WS 相关路径的局部测试全绿；`pause_turn` 不冒充完成、无参工具不丢、半截工具不能先提交成功、空参数只在精确协议位置归一 |
| R3-2 目录与计量 | AG-N12、KI-N5，P1 | 并发倒序、token refresh generation、账号删除/重绑、成功空目录、stale、不同时代 CAS 全测；usage 明确上游值优先，context fallback 的 shadow/receipt 证明后才切权威，不双计输出/缓存 |
| R3-3 差分后决策 | AG-D3、CL-N7、CX-D3/D4、CB-D3，P1/P2 | 每项单独冻结绿/红；绿灯只补 fixture/evidence，红灯只改失败形状并加 decoy/mutation；不得据外部实现重构共享 4 万行 forwarder |
| R3-live | GR-N4、KI-N3/共享 cache、既有 AG/Claude/Codex/Cursor/Grok/Kiro/Qoder/CodeBuddy operation gates | fresh、脱敏、精确 Account/Provider/rail/site/model/operation receipt；一个 rail/站点/模型的通过不可外推，缺凭据仍 `live_pending` |
| R3-Q | TokenRouter 最新增量 | `project-doc` skill 真正可用且仓库指令完整遵守后，仅读已提交业务对象并独立写 QD 增量结论；目前没有完成时限或“无变化”的替代结论 |

本轮只改计划。将来动生产协议/能力时，需同时核对或追加 `PROTOCOL_EVIDENCE.md`、合同源、reference-delta 和 Provider Registry/UI matrix；`docs/provider/coverage.md` 是从 `assets/contract/provider-coverage.json` 生成的文件，禁止手改。正常实现验收按第 14 节运行相称的 Rust/Node/静态检查和本地 smoke；本轮及后续均不做 UI 自动化和部署测试。外部项目始终只是一次性只读研究证据，绝不进入构建、运行、CI 或同步依赖。

### 18.4 第三轮完成定义与未解决门禁

1. 静态缺口须先有会失败的精确 fixture，实施后再有成功/错误、stream/non-stream、decoy、bounded-memory、取消和身份漂移回归；当前均只是**计划项**，不能标记 `fixture_verified`。
2. `differential_first` 须留可审计的绿/红记录；没有红灯不改生产；明确不采纳账号池、动态未验证客户端指纹、伪造签名、静默丢坏工具、无 receipt 开 Compact/cache。
3. 任何能力、目录或 usage 变化不得把 stale 当实时 entitlement，不能越过 Provider/Account/Share/身份代际，也不能把参考项目的估算写成收费权威。
4. Cursor、grok2api、sub2api 的本次 no-new-wire 是**提交态差分结论**，不是两条 Cursor rail 或 Grok operation 的真实验收；CodeBuddy 共享流修复同理。
5. Qoder 最新 TokenRouter 仍为 `unreviewed_skill_gate`。只有获取并遵守必需的 `project-doc` skill 后才能完成八类最新源码的全部审计；该门禁不得靠本节其余七类完成来抹平。

### 18.5 实施流水账（追加，不改基线判定）

2026-10-08，Claude 单类别开始。CL-N8 的 JSON 和 SSE fixture 均先在目标基线复现 `completed` 红灯；当前工作树以一个共享 terminal-details helper 修复为 `response.incomplete` / `status=incomplete` / `incomplete_details=null`，并保持原生 Claude/Chat 语义。`anthropic_pause_turn` 2/2、`anthropic_` 128/128、`claude` 270/270、`cargo check`、rustfmt 通过；`PROTOCOL_EVIDENCE.md` 已记录来源和本地证据。尚未有提交态 implementation tree 或真实 receipt，故只标记 `local_fixture_verified_uncommitted`。Claude 类别未结束：CL-N7 续线程 alias、leading-user/cache/tool-change 差分和相关参考证据仍待完成，下一轮继续 Claude，不转入其他 Provider。

同日继续 CL-N7：按 `CLIProxyAPI@678a9257` 的成功响应 message ID→alias 差分，在 Server 内独立落地精确 Claude OAuth thread 状态。其作用域额外限定 Share、用户、显式 session、Provider revision/runtime 和 Account auth identity generation，容量/TTL 有界；缺声明续接无状态时发网前 404，不用首条用户文本 hash 代替线程身份。JSON 成功语义和 SSE `message_stop` 才提交下一 message ID，失败/终态前取消不提交；历史 tool use/reference 与响应名恢复共用映射。非流 loopback 及流式成功续接、错误 session、过期/淘汰/代际 decoy 已有本地 fixture；真实 Claude OAuth thread shape/beta 与跨进程连续性仍待私有 receipt，当前只标 `local_fixture_verified_uncommitted`。Claude 类别尚余 leading-user/cache/tool-change 差分，不转入 Codex。

Claude leading-user/cache/tool-change 差分继续：目标当前第三方 caller system 始终插在首个 user turn 前，leading-user run 与 single terminal user 由本地 fixture 锁定；参考 cloaking 的位置规则不直接迁入。对 `CLIProxyAPI@26e5efbb` 和 `@958eeca7` 两项静态红灯则窄修：存在非 null `thread` 时把显式 cache-control 断点上限从四降为三，给线程续接保留一个；`tool_addition`/`tool_removal` 的已声明 reference 与非 server inline definition 共用 alias 映射，文本、未声明及 server tool 不改。流/非流均经同一请求准备路径。此处仅为离线合同，缺真实账号 receipt 时不声称 Claude 类别完成。

局部验证已达到 Claude 关键词 279/279、Anthropic 关键词 129/129，`cargo check`、Clippy、rustfmt、Provider coverage/UI matrix 和 Claude wire/reference-delta 审计通过。后续在提交态还需冻结实现 tree，并以真实 Claude OAuth account 对 thread shape/beta 与 cache/tool-change 接受性留存私有 receipt；没有凭据时继续 `live_pending`。

共享路径全量 Rust 单测另以 64 MiB 测试栈完成：3256 passed、1 ignored、0 failed。Claude 本地实施差分到此闭环，下一类按顺序进入 Codex；“本地闭环”不等于 Claude 真实 operation 验收，所有缺失 receipt 仍按原 gate 保留。

2026-10-08，Codex 单类别继续。CX-N13 的空/空白历史 `function_call.arguments` 精确 fixture 先红后绿；HTTP、Compact、WebSocket `response.create` 与 WS→HTTP fallback 共用准备逻辑，非空坏 JSON、custom tool 和普通用户字段不动。CX-D3 的已知整数工具字段差分也先红后绿：只在官方形态 Codex CLI/TUI 请求进入非 Codex Provider 目标时，将固定工具直接属性的 schema `number` 收窄为 `integer`；普通 number、嵌套字段、namespace/`additional_tools` 之外的输入、Codex→Codex 和假客户端均有 decoy。两项均在工作树，尚未冻结 implementation commit/tree。

CX-D4 对首帧前 EOF 的 loopback 新增 502 断言；首事件超时、Chat EOF 缺 `finish_reason`、citation/search 与 tool round-trip 由目标现有共享合同对照，未发现需要放宽终态或新增 Codex 私有桥接的红灯。Codex 关键词本地回归 406/406，全量 Rust library 3261 passed / 1 ignored；`cargo fmt --check`、`cargo check`、Clippy `--all-targets -D warnings`、Provider contract/coverage/UI matrix、Codex reference-delta 默认与 committed-source 审计均通过。真实 Codex OAuth operation 的精确 Account receipt 仍缺失，保持 `live_pending`，不以 loopback 冒充 live。下一类别按顺序为 Cursor；本轮不做 UI 自动化或部署测试。

同日 Cursor 单类别复核：推荐的 OmniRoute 已提交 HEAD/tree 仍为 `443d66996d69` / `8d1c312f61d1`，没有新的 committed delta；其 22 项未提交 Codex 文件继续全部排除。目标 Cursor 专用代码无本轮差分，`CUR-N2=reviewed_no_wire_delta` 保持，不制造生产修复。当前工作树的 Cursor Rust 关键词 304/304、双 rail receipt Node 门禁 21/21、reference-delta `--check-sources`、Provider coverage/UI matrix 均通过。`RUN_REAL=0` 的 OAuth 与 API-key harness 分别返回 `blocked_inputs/live_pending`，不触发网络；两条真实 rail 都缺必需私有输入与 receipt，继续独立 `live_pending`。这不是部署验收，下一类别按顺序为 Grok。

同日 Grok 单类别继续：grok2api 的已提交增量仍只有 README/图片，sub2api 无新提交且六项工作树改动排除。GR-N4 的 live-only 版本决策不变；但在目标静态对照中发现账号 `extra_headers` 可手工注入尚未获准的 `x-grok-conv-group-id`。先用受控头 fixture 复现红灯，再把它加入仅 Grok OAuth 的账号头拒绝名单，并验证绑定账号实际合并在发网前 400、默认 CLI profile 仍不发送 group header；`0.2.111` 常量与 override 均不改。Grok 关键词 226/226、三项 operation receipt Node 门禁 11/11、reference-delta `--check-sources`、Provider coverage/UI matrix 通过。`RUN_REAL=0` 的 inference/media/remote_compaction 全是 `blocked_inputs/live_pending`；泛化 `version_gate=pass` 标志不构成 GR-N4 所需的同账号 catalog/HTTP/WS 版本及 header receipt。此局部边界修复仍未提交，真实 GR-N4 和三项 operation 保持独立 pending，下一类别为 Kiro。

验证补记：并行产生的 Codex-only 提交 `ac69ecb` 与本轮 Grok 工作树改动分开保留；在该最新 HEAD 上重跑全量 Rust library 3263 passed / 1 ignored，`cargo fmt --check`、`cargo check`、Clippy `--all-targets -D warnings` 和 Grok 已提交源码审计均通过。这些仍是本地离线证据，不构成部署或真实账号验收。

2026-10-08，Kiro 单类别继续。只读参考 `kiro.rs@9066e9c4`（tree `5d22f040`）的 `93459b5` 无参工具恢复和 `700fbdb`/`baacf84`/`dbb8d9c` context usage 扣减，源路径、提交态 SHA-256 和目标起点见 `PROTOCOL_EVIDENCE.md`。KI-N4 先用 stream/non-stream fixture 复现红灯，再在 EventStream `endEvent` 后只恢复零字节、未 stop 的工具为 `{}`；按 ID 稳定排序，保持请求内名称回映和桥接；任一非空半截参数在成功终态前失败。既有 `stop=true` 合法工具和已停止坏 JSON 的局部宽容策略不扩张。KI-N5 仅用 `context=80/output=9` 得到候选 `71` 的 shadow 对照，明确 metrics/tokenUsage 及缓存零值优先；因八个 auth-kind × region scope 缺真实 receipt，生产 usage 仍报告原值，保持 `live_pending`，绝不以参考计算取代收费权威。

本地 Kiro 关键词回归 187/187、Amazon Q 14/14、Kiro receipt 门禁 5/5 通过；`cargo fmt --check`、`cargo check --lib`、Clippy `--all-targets -D warnings`、Provider contract/coverage/UI matrix 和既有 Kiro reference-delta `--check-sources` 均通过。本轮工作树尚未冻结实现 commit/tree，故 reference-delta v2 的新 committed-target observation 仍待提交态收口；当前审计通过仅复核既有冻结记录。Kiro/Amazon Q Compact 和共享 cache 继续 disabled，不做 UI 自动化或部署测试。下一类别为 Qoder；其最新参考仍受 `project-doc` skill gate 约束，不能把未审核增量误记为完成。

同日 Qoder 单类别继续：`TokenRouter@6d676f1d` 的 `project-doc` 门禁仍未满足，不读取其业务对象，也不运行外部源码 `--check-sources`；最新增量保持 `unreviewed_skill_gate`。只检查本仓库三 rail receipt harness，发现 `message_stop` / `response.completed` / Gemini finishReason 后仍可接收 error/failed 帧并写通过 receipt，以及 Codex 过早 `[DONE]` 仍可通过。先用四个 loopback fixture 复现红灯，再让共享 SSE 收集器读至 EOF 后拒绝任何 error/failed/incomplete 及过早/重复 `[DONE]`，同时保留完成后单个最终 `[DONE]` 的正常路径。`scripts/smoke/qoder-real.mjs` 与测试工作树改动只提高证据门禁，不改变生产 COSY wire、receipt schema v2/revision 2 或三条 rail 的 `live_pending`。Qoder receipt Node 13/13、CLI oracle、默认 reference-delta、Provider coverage/UI matrix 和 `git diff --check` 均通过；真实凭据仍缺，禁止将 fixture 解释为 live。下一类别为 CodeBuddy。

同日 CodeBuddy 单类别 CB-D3 差分：`cli2api@ead3202` 的流完成后断连计成功修复在当前已提交 HEAD 的核心收尾文件仍有效，外部未跟踪文档不作证据。目标原有 `CodeBuddySseDecoder` 只有唯一 `[DONE]` 且 EOF 才置 terminal；Intl/CN × Claude/Codex/Gemini 的提前断连矩阵验证 `client_cancelled`、`Interrupted` 及 body/租约释放，正常全量消费记录 `completed`。但新增“下游刚取走最终 terminal chunk 就停止 poll”的精确 fixture 红灯：CodeBuddy EOF 分支把成功账务放在 `yield` 后，guard 会误记取消。只在 CodeBuddy 的该 EOF 分支把 `finish_qoder_stream_success` 提前到交付最终 chunk 前；所有 EOF、唯一 `[DONE]`、runtime generation、transform 检查仍先完成。Claude/Codex × Intl/CN 的最终 chunk 断连转绿；Gemini `finishReason` 可早于 EOF，继续只能在真正 EOF 后成功。CodeBuddy 关键词 71/71（64 MiB 测试栈）、双站 receipt Node 6/6、committed-source audit、`cargo fmt --check`、`cargo check --lib`、Clippy `--all-targets -D warnings`、Provider coverage/UI matrix 均通过。改动尚未提交，Intl/CN 真实 receipt 与 CB-N4 均保持 pending/disabled。此后进入整体 review，不做 UI 自动化或部署测试。

同日第三轮整体 review：全量 Rust library 3269 passed / 1 ignored，audit/smoke Node 单测 163/163；七类允许的 reference-delta `--check-sources`、Qoder 默认自包含 audit、Provider coverage/UI matrix 和文档索引通过，Qoder 最新业务源码依旧未审。发现 Claude thread alias 缓存只按 alias 文本计字节，未把两份作用域键和消息 ID 纳入 64 KiB/8 MiB 约束；超长 session fixture 先红后绿，现按真实 retained key/entry/alias 容量拒绝过大写入。`state.rs` 增加 Claude 状态域方法使 Phase 0 Provider writer 清单哈希过期，已只更新其完整源码 SHA-256；相关 Node 测试 6/6 恢复通过。以上是工作树级本地证据，不等于新 implementation commit/tree 或真实账号验收。

完成审计判定为**未完成**：AG-N12 的倒序目录、token generation 与成功空目录 CAS 未实施；AG-D3 的双层 CAQS、context hash、多图/并行工具和断连差分尚无结果；KI-N5 仍是 shadow/live receipt 前不改计量；Qoder 最新 HEAD 仍受 `project-doc` skill 门禁；各 Provider 的真实 Account/rail/site/operation receipt 仍独立缺失。下一轮应只处理 Antigravity 的 AG-N12/AG-D3，随后按单类别循环收口其他剩余项；等提交态 observation 和必需真实门禁齐备再做最终整体 review。继续禁止 UI 自动化和部署测试。

同日 Antigravity 单类别接续（不追改上条历史审计快照）：AG-N12 的 A/B 倒序 loopback 现覆盖旧 token 挂起、新 token 成功空目录先发布、旧调用晚返回 409，旧逐模型 search entitlement 清除；额外缓存 fixture 覆盖旧 auth generation 的失败清理不能删除新 key，以及新 token 代际优先于旧调用较大请求序号。缓存键加入 token refresh generation、项目和 URL，请求发布 revision 防旧成功结果覆盖新成功；API/状态证据写入再核对账号两代 generation、项目和最新持久化 revision，项目漂移的旧结果不得写证据。相关工作树尚无 implementation commit/tree 或真实账号 receipt，状态暂记 `local_fixture_verified_uncommitted`，不能提升 live。

AG-D3 本轮只补部分绿灯：双层 CAQS opaque round-trip、多图顺序变动导致 context hash 拒绝、并行 tool/result 完整性、terminal 前断连不缓存已有或新增 fixture；参考 Manager 的双层 base64 判别不等于目标必须改写 Google/Claude 签名。思考预算与模型后缀优先级、terminal 后下游断连仍待单独差分及必要的 live receipt，故 AG-D3 维持 `partial_fixture_verified_uncommitted`，不称完成。`PROTOCOL_EVIDENCE.md` 记录参考 HEAD/tree、目标基线与边界；不引入账号池、不做 UI 自动化或部署测试。下一轮先收 AG-D3 未结项，再继续其他独立门禁；八类总体目标仍未完成。

本轮验证补记：Antigravity 关键词 74/74、全量 Rust library 3274 passed / 1 ignored；Provider contract/coverage/UI matrix、Phase 0 清单 6/6 和已有 Antigravity reference-delta committed-source 审计通过。`src/api/providers.rs` 与 `src/state.rs` 的 Phase 0 源码摘要已同步；新增实现尚未提交，不能伪造 v2 的 committed-target observation。AG-N12 仍需精确删除后复用同 ID/同代际账号的生命周期反例，AG-D3 仍有上述未结差分，当前目标不宣称最终完成。

同日 Antigravity 再接续（前述待办按此最新记录更新）：AG-N12 的同 ID 删除重建边界已用进程内 account incarnation 精确覆盖。即使新账号复用相同 ID、token、auth/token generation，旧请求的缓存发布与能力证据都被拒绝；正常刷新不换 incarnation，进程重启不保留在途请求，故该字段不进入持久化/API。AG-N12 本地设计与回归现收口为 `local_fixture_verified_uncommitted`，真实账号删除/重绑及 `antigravity_oauth`/`agy_oauth` receipt 仍 `live_pending`。

AG-D3 的固定预算大于输出上限先红后绿：仅在 Antigravity 最终 wire 边界对显式固定整数 `thinkingBudget >= maxOutputTokens` 返回 400，保持调用者输出上限不被自动抬高；自适应 `-1`、`thinkingLevel` 和缺上限输入不猜默认值。已有 `antigravity_stream_commits_replay_at_terminal_before_upstream_eof` 是 terminal 后下游断连的精确绿灯。Registry 的显式 `single/passthrough` 模型绑定优先，CLIProxyAPI 的 `model(high)` 通用后缀覆盖 body 不在缺真实目录/receipt 时迁入，记 `not_adopted_pending_live`；双层 CAQS 不猜签名编码。AG-D3 计划的离线差分形状已逐项有绿/红/不采纳结论，状态 `local_differential_complete_uncommitted`，不等于厂商 live 验收；两条 rail 的真实 receipt、提交态 v2 observation 仍独立待办。八类总体实施与最终整体 review 仍未完成。

Antigravity 单类别本地收尾复核：`cargo test` 完整执行通过，library 3276 passed / 1 ignored，另有 124 + 1 + 1 项非 library 测试通过；`cargo clippy --all-targets -- -D warnings`、`cargo fmt --check`、`git diff --check`、Provider Phase 0 / Server Provider contract / coverage / UI matrix、文档索引及 Antigravity reference-delta `--check-sources` 均通过。上述审计只核对既有 committed-source observation，当前工作树的 AG-N12/AG-D3 实现仍未提交，因此不能补写假的 committed-target v2 observation。Antigravity 的离线实施本轮收口；真实 `antigravity_oauth`、`agy_oauth` 的精确 Account/模型/operation receipt 仍 `live_pending`，八类总体目标与最终整体 review 仍未完成。下一单类别应继续核对 Claude 的剩余提交态/真实门禁，不把离线绿灯当作 live 通过；不做 UI 自动化或部署测试。

同日 Claude 单类别增量复核：CL-N7 在“续接时添加新工具，但历史仍引用旧非法名称”形状下有额外红灯。原逻辑只在 `tools` 为空时加载上一 message ID 的别名，因此三段 create→带新声明的 continue→无声明 continue 会丢旧映射；第二段上游返回旧 alias 时下游还原失败。新增本地 loopback 先复现，再在既有精确 Share/用户/session/Provider/runtime/Account 身份作用域内可选合并旧、新映射；wire/原名碰撞和超过 128 条在发网前拒绝，成功终态才把并集传给下一 ID。无状态且有完整声明的续接兼容行为不变；无声明且缺状态仍 404。thread-alias 10/10、Claude 关键词 282/282、Claude wire/profile/reference-delta 及 Provider Phase 0 / contract / coverage / UI matrix 审计通过。此增量仍为 `local_fixture_verified_uncommitted`，五个真实 Claude operation 与 thread/tool-change 接受性继续 `live_pending`；整体目标尚未完成，不做 UI 自动化或部署测试。

Claude 单类别本地收尾复核：完整 `cargo test` 为 library 3278 passed / 1 ignored、非 library 124 + 1 + 1 passed；Clippy `--all-targets -D warnings`、rustfmt 和私有 receipt harness 离线 Node 13/13 通过。`RUN_REAL=0` 检查五个 operation 均返回 `blocked_inputs/live_pending`，未联网、未产生真实 receipt。当前工作树未提交，新增 CL-N7 差分不能伪写为 committed-target v2 observation；离线实施可转下一类别，但五个真实 operation 和 thread/tool-change 厂商接受性仍需独立验收，最终整体 review 不得据此标完成。

同日 Codex 单类别复核：推荐的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 仍为第 18 节已审提交态，无新 committed delta。重新核对 CX-N13 在 Responses HTTP/Compact、WebSocket `response.create` 及 HTTP fallback 共用的最终清理入口，以及 CX-D3 只对官方形态客户端→非 Codex executor 的已知整数参数字段生效；CX-D4 的首帧前 EOF、fallback 与终态用例仍在目标。没有新红灯，不制造额外 wire 改写。当前工作树 Codex 关键词 Rust 407/407、私有 receipt 门禁离线 Node 13/13、Codex reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix 均通过。`RUN_REAL=0` 对 `gpt_6_astra`、三个 `gpt_image_2_5` exact-model operation 与 `ws_prewarm` 分别返回 `blocked_inputs/live_pending`，没有请求真实服务；账号、Provider/Share/模型和仓库外私有 receipt 均未提供。CX-N13/CX-D3 仍是未提交工作树实现，不能伪造 committed-target v2 observation；五个真实 operation 独立 pending。下一单类别转 Cursor；不做 UI 自动化或部署测试。

同日 Cursor 单类别增量：OmniRoute 推荐参考的已提交 HEAD/tree 不变，22 项未提交 Codex 工作树内容继续排除，CUR-N2 的 no-new-wire 结论保持。独立检查本仓库发现 API Key 动态目录的发布栅栏缺口：`append_cursor_api_key_models` 用旧 Provider 快照，等待期间同一 Provider 凭据轮换后仍可把旧作用域的缓存目录写入响应。旧快照 + 旧目录 + 新凭据的确定性 fixture 先红后绿；现在进入响应前复核 Provider 类型/revision、credential generation、driver/runtime fingerprint 与静态凭据代际，漂移则跳过该目录，不引入账号池或更改 OAuth/protobuf/session wire。Cursor 关键词 305/305、完整 Rust library 3279 passed / 1 ignored、非 library 124 + 1 + 1 passed；Clippy、rustfmt、Cursor receipt Node 21/21、reference-delta `--check-sources`、Provider Phase 0 / contract / coverage / UI matrix 通过。`src/api/mod.rs` 的 Phase 0 compatibility 源码摘要已由生成脚本同步；工作树尚无提交态 implementation tree，不能伪造 v2 observation。OAuth/API Key 两条 `RUN_REAL=0` 门禁分别返回 `blocked_inputs/live_pending`，真实 receipt 独立缺失。Cursor 离线增量到此收口；下一类别为 Grok，最终整体 review 仍未完成，不做 UI 自动化或部署测试。

同日 Grok 单类别接续：推荐的 `grok2api@7c889a96` 只有已审提交态增量，`sub2api@ab99d56e` 无新提交且六项未提交 Codex 文件继续排除。目标自身的旧 token 目录请求晚返回会删除新 token 已发布的模型/能力缓存；同 ID 删除重建也不能单靠 ID 和 auth/token generation 区分账号实例。两个精确回环反例先红后绿：目录 scope 新增进程内 account incarnation，发布时比较账号实例、Provider revision、auth/token generation 与请求序号；API 返回前再核对账号实例和缓存 revision，旧结果 409 拒绝。`0.2.111`、group header、账号选择和推理 wire 均不变。

Grok 关键词 230/230，完整 `cargo test --quiet` 为 library 3282 passed / 1 ignored、非 library 124 + 1 + 1 passed；Grok receipt Node 11/11、reference-delta `--check-sources`、Provider Phase 0 / contract / coverage / UI matrix、rustfmt、Clippy `--all-targets -D warnings` 和差分空白检查通过。`RUN_REAL=0` 的 inference、media、remote_compaction 仍分别为 `blocked_inputs/live_pending`。当前新增实现尚在工作树，缺 implementation commit/tree，不能追加假的 v2 committed-target observation；GR-N4 的同账号版本/header 接受性和三项 operation 的真实 receipt 仍独立缺失。本类别离线增量到此收口；下一类别为 Kiro，八类总体目标与最终整体 review 尚未完成，继续禁止 UI 自动化和部署测试。

同日 Kiro 单类别接续：推荐的 `kiro.rs@9066e9c4` 没有新提交。复核 KI-N4 时发现目标对未显式 stop 的纯空白工具参数，收尾分支与已支持纯空白的 `stop=true` 分支不一致。双 Surface fixture 先红后绿；当前工作树在 `endEvent` 收尾把纯空白恢复为 `{}`，仍以 `TOOL_JSON_INCOMPLETE` 拒绝含非空白的半截 JSON，不改变工具名作用域、容量上限、终态或路由。Kiro 关键词 188/188；全量 Rust 在 64 MiB 测试栈为 library 3283 passed / 1 ignored、非 library 124 + 1 + 1 passed。默认栈下既有 CodeBuddy 测试发生 stack overflow，不能将首次全量运行写成绿灯；Kiro receipt Node 5/5、reference-delta `--check-sources`、Provider contract/coverage/UI matrix、Clippy `--all-targets -D warnings` 和 rustfmt 通过。

KI-N5 仍只保留 context usage 扣除输出的 shadow 对照，缺八个 auth-kind × region 的绑定账号 receipt，不改生产计量；Compact 和 shared cache 继续 disabled。Builder ID / `us-east-1` 的 `RUN_REAL=0` 门禁为 `blocked_inputs/live_pending`，其余七份 receipt 也仍缺失。本轮 KI-N4 增量未提交，不能新增假的 committed-target v2 observation；Kiro 离线修复收口后下一类别为 Qoder，最终整体 review 尚未达到完成条件，禁止 UI 自动化和部署测试。

同日 Qoder 单类别接续：最新 TokenRouter 的 `project-doc` 技能门禁仍未满足，业务增量保持 `unreviewed_skill_gate`，不读取外仓业务源码或运行其 source audit。本仓库三 rail 私有 receipt harness 的 Claude/Codex/Gemini × 流/非流工具检查却有独立红灯：六种返回 `wrong-key` 的 mock 都能凭工具名 `lookup` 写出 `tool=pass`。现只收紧 `scripts/smoke/qoder-real.mjs` 的证据门槛：验证工具 ID/完整参数和唯一预期 key，Claude SSE 合并分片并等待 block stop，Codex 使用完成态函数参数，Gemini 核对 `functionCall.args`；六个负例转绿，合法 Claude 分片正例与三 rail 正常 fixture 保持通过。既有 receipt schema v2/revision 2、生产 COSY wire、账号/站点/rail 选择和能力开关均未改。

Qoder receipt Node 21/21、CLI oracle、默认 reference-delta、Provider contract/coverage/UI matrix、脚本语法与差分空白检查通过。Global OAuth、Global PAT、CN OAuth 的 `RUN_REAL=0` 均各自为 `blocked_inputs/live_pending`，真实 receipt 缺失；默认审计不能代替受门禁的最新 TokenRouter 源码审计。新增证据门槛尚未提交，不追加假的 committed-target v2 observation。Qoder 本轮离线增量收口后下一类别为 CodeBuddy；整体目标与最终 review 仍未完成，禁止 UI 自动化和部署测试。

同日 CodeBuddy 单类别接续：推荐 `cli2api@a6502521` 未带来新 CodeBuddy wire 结论，两个未跟踪文档仍排除。独立审查发现私有 receipt 校验器只在开始读取一次 Account、三 Surface Provider 和 Share；其后目录请求期间若 token generation、Provider revision 或 Share revision 漂移，旧 scope receipt 仍会被接受。Intl/CN × 三类漂移的六个 mock 先红后绿；现在在 receipt 内容验证后再取一次完整绑定快照，重算 generation/binding digest 并与起点比较，漂移即失败关闭。正常两站 fixture 各控制面资源读取两次仍通过。这不是跨 API 的原子事务，也不改变生产转发、站点绑定、receipt schema v2/revision 2 或 CB-N4 的禁用决策。

CodeBuddy receipt Node 13/13、reference-delta `--check-sources`、Provider contract/coverage/UI matrix、脚本语法和差分空白检查通过；Intl/CN 的 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，真实私有 receipt 仍缺。本轮新增验收逻辑未提交，不能追加假的 committed-target v2 observation。CodeBuddy 离线增量暂收口后应进入跨类别整体 review，逐项核对仍缺的真实门禁与提交态证据；总体目标未完成，禁止 UI 自动化和部署测试。

### 18.6 第三轮跨类别整体复核（中期，非最终完成）

2026-10-08 当前 HEAD 为 `ac69ecb1c652c0ac924394203617df3b57fa2498`，工作树有 29 个已跟踪文件改动和 1 个未跟踪实现文件。第 18.5 节中的新修复、receipt gate 与文档尚未形成新的 implementation commit/tree，因此现有 append-only v2 audit 通过只证明**既有已提交 observation**和冻结外部 Git object 未漂移，不能证明这 30 个工作树路径已有 committed-target observation，也不能把 fixture 升级为 live。

| 类别 | 本轮可证明的本地进展 | 仍未关闭的独立条件 |
| --- | --- | --- |
| Antigravity | AG-N12 目录发布/同 ID 重建栅栏、AG-D3 列举差分已有先红后绿或明确不采纳记录 | 两条 OAuth rail、AG-N6/requestType 与 compaction 的真实接受性；新增实现的提交态证据 |
| Claude | CL-N7 续线程 alias、CL-N8 `pause_turn`、cache/tool-change 差分已离线验证 | 五个 operation 的真实 receipt，thread/beta 厂商接受性；新增实现的提交态证据 |
| Codex | CX-N13 空函数参数、CX-D3 精确整数 schema、CX-D4 终态差分已离线验证 | `gpt_6_astra`、三个 Image 2.5 模型和 WS prewarm 各自真实 receipt；新增实现的提交态证据 |
| Cursor | 推荐参考无新 committed wire；目标 API Key 动态目录旧 Provider 栅栏先红后绿 | OAuth/API Key 两 rail 真实 receipt；新增实现的提交态证据 |
| Grok | group-header 注入防护、目录旧 token/同 ID 重建发布栅栏先红后绿 | GR-N4 同账号版本/header 证据及 inference/media/remote_compaction 三项 receipt；新增实现的提交态证据 |
| Kiro | KI-N4 无参/纯空白工具终态恢复先红后绿 | KI-N5 context usage 仍仅 shadow；八个 auth-kind × region receipt、Compact/shared cache 独立启用证据及提交态证据 |
| Qoder | 三 rail receipt 的错误终态和工具参数假阳性已先红后绿 | TokenRouter 最新业务增量受 `project-doc` 技能门禁，三 rail 真实 receipt 和新增验收逻辑提交态证据 |
| CodeBuddy | CB-D3 最终 chunk 断连账务、Intl/CN receipt 末次绑定复核已先红后绿 | Intl/CN 真实 receipt、CB-N4 厂商目录/接受性，以及新增实现/验收逻辑提交态证据 |

合同现态逐份核对：八类 reference-delta 的 `realAcceptance` 主体共有 28 个 `receipt=null/live_pending`；Claude `opus_5_5` 与 Codex `gpt_6_astra` 分别在追加的 `realAcceptanceExtensions` 中各占 1 项，总计 **30 个互不继承的真实验收项**。此外 AG-N6、GR-N4、KI-N5、KI-N3/共享 cache、CB-N4 等决策仍需其专项证据或继续保持 disabled，不能把 30 个 receipt 简化为一个总开关。

本轮验证范围是当前工作树的离线回归：`RUST_MIN_STACK=67108864 cargo test --quiet` 为 library 3283 passed / 1 ignored，非 library 124 + 1 + 1 passed；`node --test scripts/audit/*.test.mjs` 为 165/165，`node --test scripts/smoke/*.test.mjs` 为 13/13。`cargo fmt --check`、Clippy `--all-targets -D warnings`、Provider Phase 0/Server contract/coverage/UI matrix、文档索引、`git diff --check` 均通过。Antigravity、Claude、Codex、Cursor、Grok、Kiro、CodeBuddy 七类 reference-delta 的 `--check-sources` 通过；Qoder 只运行不访问最新外仓的自包含默认 audit，不能把该绿灯写作最新 TokenRouter 已审。上述检查不覆盖真实厂商接受性、部署行为或 UI 自动化；后两种测试明确未执行。

因此本次是**中期 review，完成判定仍为未完成**。下一步按单类别继续处理还能离线复现的缺口；提交态 evidence 应在明确提交形成真实 Git object 后逐类追加，禁止伪造；真实账号/Router/Share/站点/operation 输入到位时逐份执行私有 receipt，不得外推。Qoder 最新源码必须等 `project-doc` 真正可用再合规补审。全部规划项与真实门禁均有权威证据后，重新做一次最终整体 review，才可考虑标记目标完成。

### 18.6 Antigravity 私有真实验收门槛 v2 接续

2026-10-08，按单类别继续 Antigravity。目标旧 `scripts/smoke/antigravity-real.mjs` 允许任意久远或超前的正数目录时间戳及任意可解析 receipt 日期；首次读取的 Account、两个 Provider 和 Share 在 receipt 接受前也不复核。两条 OAuth rail 的目录/receipt 时间异常 8 个、Account token generation / Provider revision / Share revision 漂移 6 个离线反例先红后绿。验收现要求目录不超过 2 小时、receipt 不超过 24 小时，最多容忍未来 5 分钟时钟偏差，并在接受前重读精确绑定。这是控制面末次快照栅栏，不是跨 API 原子事务，亦未改生产 Antigravity wire。

旧冻结 `realAcceptance` 的 schema/revision 1 保持不变；追加 `AG-LIVE-V2-2026-10-08` 扩展，当前 harness 只接受 schema 2 / revision 2。v2 scope digest 与 receipt 快照同时绑定两份目录的获取时间和完整目录内容 SHA-256。即使所选模型 ID 不变，目录多出一项，或 receipt 自身目录 digest 被篡改，两条 rail 共 4 个新增负例也会拒绝，不再把旧目录摘要当作当前目录。正常 fixture、时间/漂移及内容篡改测试合计 28/28；参考源对象、Provider Phase 0/coverage/UI matrix、文档索引、脚本语法与 `git diff --check` 通过。此为离线验收合同，不是 Google live 接受性；`RUN_REAL=0` 的两条 rail 均仍 `blocked_inputs/live_pending`，真实 receipt 均缺失。AG-N6/requestType、compaction 和新增逻辑的 committed-target observation 仍各待独立证据，不能沿用中期 review 的 30 项总表将其判完成。本次未提交工作树，未做 UI 自动化或部署测试。Antigravity 此项离线门槛收口；下一类仍按单类别推进，最终整体 review 尚未完成。

### 18.7 Claude 私有 receipt 末次绑定复核

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568` 的已提交 HEAD 不变；这次发现的是目标自身五个 operation 验收器的时间窗口，而非新的上游 wire。原流程开始读取 Account/Provider/Share，查模型和可选套餐后直接接受 receipt。五个 operation × Account token generation、Provider revision、Share revision 共 15 个回环反例先红灯：绑定已漂移，旧 scope receipt 仍被接受。现在 receipt 内容校验后再读取精确绑定，比较 Account ID/两代 generation、Provider revision/runtime fingerprint、Share revision；任何漂移均失败关闭。五个正常 fixture 仍通过，控制面资源各读取两次，Claude receipt Node 29/29。

该末次快照不是跨 API 原子事务；既有 receipt schema v1/revision 1、套餐 projection、生产 Claude wire、账号选择与恢复预算均未改变。Claude reference-delta `--check-sources`、wire profile、Server Provider contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过；全量 Node audit 202/202。五个 `RUN_REAL=0` 入口分别为 `blocked_inputs/live_pending`，真实 operation receipt 仍 `null/live_pending`；当前新增验收逻辑在未提交工作树，不能伪造 committed-target observation。Claude 此项离线门槛收口，下一类别继续按单类别处理；整体目标和最终 review 尚未完成，不做 UI 自动化或部署测试。

### 18.8 Codex 私有 receipt 末次绑定复核

2026-10-08，Codex 单类别接续。推荐的 `CLIProxyAPI@0f96f568` 和 `codex2api@b1bfe1b4` 提交态不变；这次是目标自身证据验收器缺口，不是新的 OpenAI wire。五个 operation 在首次读取 Account/Provider/Share 后查可选模型目录并解析 receipt，旧逻辑不再确认绑定。五类 operation × Account token generation、Provider revision、Share revision 共 15 个回环反例先复现错误接受。receipt 校验后增加同一精确绑定的末次快照，比较 Account ID/两代 generation、Provider revision/runtime fingerprint 与 Share revision；漂移即失败关闭，五个正常 fixture 保持通过且各控制面资源读取两次。Codex 私有 receipt Node 29/29。

此栅栏不保证跨 API 原子性；receipt schema v1/revision 1、生产 HTTP/WS/image wire、模型 entitlement、账号选择与恢复预算均未改。Codex reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过，全量 Node audit 218/218；官方 OpenAI Responses 流式文档不能替代固定账号真实验收。五个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`；`gpt_6_astra`、三个 Image 2.5 exact-model operation 和 `ws_prewarm` 五份真实 receipt 仍 `null/live_pending`。新增验收逻辑还在未提交工作树，不能伪造 committed-target observation。Codex 此项离线收口，下一类别仍按单类别推进，最终整体 review 未完成；禁止 UI 自动化和部署测试。

### 18.9 Cursor 双 rail receipt 与 API Key 目录门槛

2026-10-08，Cursor 单类别接续。推荐 OmniRoute 的已提交 HEAD 仍为 `443d66996d69`，22 项未提交 Codex 工作树修改不作 Cursor 证据，CUR-N2 的 no-new-wire 结论保持。本仓库双 rail receipt 验收器只在开始读取 OAuth Account、Provider、Share；随后目录与 receipt 处理期间若 OAuth token generation、API Key credential generation、Provider revision 或 Share revision 漂移，旧 scope 仍会被接受。六个精确反例先红后绿；现在 receipt 校验后末次重读同一绑定并比较账号、代际、Provider revision/runtime 与 Share revision，漂移即失败关闭。这不是跨控制面 API 的原子事务。

API Key rail 的另一组四个先红后绿反例表明，旧验收器仅凭 `*-fast` ID 出现在公共目录，就会接受 `stale=true`、超过 5 分钟、超前 10 分钟或非 `cursor_public_api` 来源的结果。现在要求精确动态目录来源、非 stale、5 分钟时限和 1 分钟未来偏差；OAuth 静态目录不套该条件。公共列表 metadata 只强化证据门槛，不能独自证明该 ID 的动态来源或厂商 entitlement，仍需两条 rail 各自真实 receipt。专项 Cursor Node 33/33，全量 Node audit 230/230；reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法与差分空白检查通过。OAuth/API Key 的 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；receipt schema v2/revision 2 和生产 protobuf/session/目录发布 wire 均不变。当前新增验收逻辑未提交，不能补假的 committed-target observation。两 rail 继续 `null/live_pending`，最终整体 review 未完成，不做 UI 自动化或部署测试。

### 18.10 Grok 三项 receipt 目录与身份门槛

2026-10-08，Grok 单类别接续。推荐的 `grok2api@7c889a96`、`sub2api@ab99d56e` 提交态未变，后者六个未提交 Codex 文件排除；GR-N4 的版本/header 决策仍不得无固定账号证据就改 wire。目标三 operation 私有 receipt 原来把任何正数 `fetchedAtMs`、任意格式合法的 `source` 当作 fresh 目录，且开始读一次 Account/Provider/Share 后不做接受前复核。三项 operation × 过旧/超前/伪来源目录 9 个，加三项 × Account token generation/Provider revision/Share revision 9 个，共 18 个离线反例先红后绿。

当前门槛只接受生产目录确实返回的 `upstream`、`upstream_not_modified`、`cache_fresh`；目录非 stale、最多 2 小时、未来偏差最多 5 分钟。2 小时是私有验收门槛，不改生产缓存 TTL；超过窗口须重新取得目录证据。receipt 校验后再读同一精确绑定，比对 Account ID/两代 generation、Provider revision/runtime fingerprint 和 Share revision。正常 cache/304 来源正例与三项 operation fixture 均通过，Grok 专项 Node 34/34、全量 Node audit 253/253。末次读取不是跨 API 原子事务。生产 `0.2.111`、group header、推理/media、目录发布与 receipt schema v1/revision 1 均未动；remote compaction 仍 disabled。reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；三份真实 receipt 与 GR-N4 专项同账号 header/version 证据仍缺，新增验收逻辑未提交，不能伪造 committed-target observation；最终整体 review 仍未完成，禁止 UI 自动化和部署测试。

### 18.11 Kiro 八路 receipt 目录与身份门槛

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4` 提交态未变。目标旧八路私有 receipt 验收器把任意正数 `fetchedAtMs` 与格式合法的任意 `source` 当 fresh，fixture 所写来源也与生产 `kiro_runtime.rs` 不符；读取两份目录和 receipt 后不重读 Account、Claude/Codex Provider、Share。四 auth kind × 两 region × 四类绑定漂移共 32 个反例，八 scope × 过旧/超前/伪来源目录共 24 个反例，合计 56 个先红后绿。

当前仅接受 `kiro_list_available_models` 或 `kiro_account_cache`、`stale=false`、两份目录均在生产 5 分钟 fresh TTL 内且未来偏差不超过 1 分钟；24 小时 stale 展示缓存不得通过。receipt 校验后重读精确 Account、两 Provider 和 Share，重算并比较包含两代账号 generation、双 Provider revision/runtime fingerprint、Share revision 的绑定摘要。八路正常 fixture 与 fresh cache 正例仍通过，Kiro 专项 Node 64/64、全量 Node audit 312/312。该末次快照不保证跨 API 原子性，生产 EventStream、KI-N5 计量权威、账号/region 路由与 receipt schema v1/revision 1 不变；Compact/shared cache 仍 disabled。Kiro reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过。八个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`；八份真实 receipt 仍 `null/live_pending`，新增验收逻辑未提交，不能伪造 committed-target observation；整体目标及最终 review 未完成，不做 UI 自动化或部署测试。

### 18.12 Qoder 三 rail receipt 末次绑定复核

2026-10-08，Qoder 单类别接续。最新 TokenRouter 业务源码仍受仓库 `project-doc` 技能门禁约束，本轮没有读取，也没有运行访问该仓的 `--check-sources`；状态继续为 `unreviewed_skill_gate`，只处理本仓库验收器。旧 harness 在开始读取 Account、三份 Provider 和 Share，三 Surface 请求后只重读 Account，且允许 token refresh generation 上升；于是 Global OAuth、Global PAT、CN OAuth 各自发生 Account token 代次、Codex Provider revision 或 Share revision 漂移时，九个回环反例均错误写出混合时点 receipt。三条正常 fixture 也证实 Provider/Share 原本各只读一次。

现在写 receipt 前重读同一精确 Account、三个 Provider 和 Share，要求 Account ID、auth identity generation、token refresh generation 完全不变，并比较 Provider binding digest、各 Surface revision/runtime fingerprint digest、Share revision/identity/binding digest；漂移即拒绝落盘。九个反例先红后绿，三条正常 rail 的三类控制面资源各读两次；Qoder 专项 Node 31/31。末次快照仍不是跨 API 原子事务，不保证复核之后的外部状态不再改变。receipt schema v2/revision 2、生产 COSY wire 与 `partial_live_verified/live_pending` 决策不变。

CLI oracle、默认自包含 Qoder reference-delta、Server Provider contract/coverage/UI matrix、文档索引、脚本语法与 `git diff --check` 均通过；全量 audit Node 322/322、smoke Node 13/13。三条 `RUN_REAL=0` 入口分别仍为 `blocked_inputs/live_pending`，真实 receipt 尚缺；本轮改动未提交，不能新增假的 committed-target observation。Qoder 离线门槛此轮收口，下一类别为 CodeBuddy；八类总体目标与最终整体 review 仍未完成，禁止 UI 自动化和部署测试。

### 18.13 CodeBuddy 双站目录 fresh 证据门槛

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521` 的已提交 HEAD/tree 未变，未跟踪 `proxy.html`、`proxy.md` 仍排除；本次不声称新的外仓 wire 差异。目标生产 `src/proxy/codebuddy_runtime.rs::CODEBUDDY_MODEL_CATALOG_TTL_MS` 规定非 stale 目录仅在 8 分钟缓存期内 fresh，超过后须重新查询或明确标为 stale；但私有 `codebuddy-real-receipt.mjs` 原允许两小时旧目录及未来五分钟时间戳。Intl/CN 两站各自的 9 分钟旧目录和超前 2 分钟目录共四个回环反例先证明旧验收器错误接受，7 分钟缓存正例保持通过。

当前验收门槛与生产 fresh TTL 对齐为 8 分钟，未来时钟偏差最多 1 分钟；接受前再次检查已绑定的三份目录仍在窗口内。四个负例转绿，双站正例保持通过，CodeBuddy 专项 Node 20/20。该改动只收紧私有 receipt 对 `fresh_catalog` 的证据，不更改生产目录 TTL、模型 allowlist、转发 wire、账号/站点路由或 receipt schema v2/revision 2；末次读取仍非跨 API 原子事务。

CodeBuddy reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法与 `git diff --check` 通过；全量 audit Node 329/329、smoke Node 13/13。Intl/CN 的 `RUN_REAL=0` 仍分别为 `blocked_inputs/live_pending`，两站真实 receipt 与 CB-N4 厂商目录/接受性仍缺。此轮逻辑未提交，不能伪造 committed-target observation；八类目标和最终整体 review 仍未完成，不做 UI 自动化或部署测试。

### 18.14 八类接续后的跨类别复核（中期，非最终完成）

2026-10-08，在第 18.6～18.13 节八类逐项接续后重新核对当前状态。目标 HEAD 仍为 `ac69ecb1c652c0ac924394203617df3b57fa2498`，工作树有 43 个已跟踪路径改动和 1 个未跟踪实现文件；没有新的 implementation commit/tree，故七类 `--check-sources` 通过只证明先前冻结的提交对象，不能为这些工作树改动生成 committed-target observation。七类推荐参考 checkout 的 HEAD/tree 与第 18.1 节一致；Qoder 最新 TokenRouter 仍因缺 `project-doc` 技能未审业务对象，只执行默认自包含 audit，不运行其 `--check-sources`。

本轮全量 `RUST_MIN_STACK=67108864 cargo test --quiet` 为 library 3283 passed / 1 ignored，另 124 + 1 + 1 passed；`cargo fmt --check`、`cargo check`、Clippy `--all-targets -D warnings` 均通过。Node audit 329/329、smoke 13/13；Provider contract/Phase 0/coverage/UI matrix、文档索引、七类 committed-source reference-delta、Qoder CLI oracle/自包含 delta 和 `git diff --check` 均通过。这些是当前工作树的离线回归，不是厂商真实接受性。

合同逐份计数：Antigravity 2、Claude 4、Codex 4、Cursor 2、Grok 3、Kiro 8、Qoder 3、CodeBuddy 2，主体共 28 个 `receipt=null/live_pending`；Claude Opus 5.5 与 Codex Astra 扩展各 1 个，总计 30 个独立真实项仍缺。Antigravity v2 扩展重述原两条 rail，不额外计数。AG-N6、GR-N4、KI-N5/Compact/shared cache、CB-N4 还各有专项决策证据要求，不能被 30 个 receipt 汇总值替代。

`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 scripts/release-readiness.sh` 返回 `verificationState=blocked_inputs`、`failures=0`：八项真实连接/凭据输入缺失、部署按禁令未测；`local-contracts-unverified` 仅因该 readiness 命令显式跳过内置测试，不能抹去上段独立完成的测试。下一步仍需在明确形成真实提交对象后追加各类 committed-target 证据、在安全提供的真实账号/Router/Share 输入下逐项留存脱敏 receipt，并待 `project-doc` 可用后合规补审 Qoder；全部证据齐备才重做最终整体 review。当前完成判定为**未完成**，没有执行 UI 自动化或部署测试。

### 18.15 Antigravity 私有 receipt 路径真实位置栅栏

2026-10-08，重新按单类别处理 Antigravity。私有验收脚本原只检查 `ANTIGRAVITY_REAL_RECEIPT_FILE` 字面路径在仓库外；一个仓库外符号链接指向仓库内、内容和 scope 均合法的 receipt，仍会被接受，违反私有证据不得放在工作树的约束。单 rail 回环 fixture 先复现错误接受，再对存在的文件解析真实路径并重复仓库边界校验，后续读取也固定使用该真实路径；现在在任何控制面或目录请求前失败关闭。两条 OAuth rail 的正常 fixture 不变，Antigravity 专项 Node 29/29。

本轮没有改生产 Antigravity wire、AG-N6 requestType、compaction、receipt schema v2/revision 2 或两条 rail 的 `live_pending`。全量 audit Node 330/330、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。`RUN_REAL=0` 两条 rail 均仍为 `blocked_inputs/live_pending`；没有真实 receipt。本次 JS 验收器改动未提交，不能新增假的 committed-target observation；整体目标及最终 review 未完成，未运行 UI 自动化或部署测试。

### 18.16 Claude 私有 receipt 路径真实位置栅栏

2026-10-08，Claude 单类别接续。`scripts/smoke/claude-real-receipt.mjs` 原只按字面路径要求 receipt 位于仓库外；外部符号链接指向仓库内、内容合法的 `oauth_inference` fixture 时仍错误接受。一个独立回环反例先红后绿；现在对存在的 receipt 解析真实路径，再次执行仓库边界校验，并从该真实路径读取，在任何控制面或模型请求前拒绝绕过。五个 operation 的正常 fixture 与既有 15 个绑定漂移反例保持通过，Claude 专项 Node 30/30。

这只强化私有证据存放约束，不改变 CL-N7/CL-N8 生产实现、Claude OAuth wire、receipt schema v1/revision 1 或五个 operation 的 `live_pending`。Claude reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过；全量 audit Node 331/331、smoke Node 13/13。五个 `RUN_REAL=0` 入口均仍 `blocked_inputs/live_pending`，真实 receipt 缺失。新增 JS 逻辑尚未提交，不能追加假的 committed-target observation；整体目标及最终 review 未完成，未做 UI 自动化或部署测试。

### 18.17 Codex 私有 receipt 控制面响应大小栅栏

2026-10-08，Codex 单类别接续。`scripts/smoke/codex-real-receipt.mjs` 原先以 `response.arrayBuffer()` 等待整个控制面响应后才检查 4 MiB 上限：服务端声明超限长度或无长度地分块发送超限内容、随后保持连接不结束时，验收器会等到超时，且上限检查前已持有超额响应。两个本地 loopback 反例先复现约 5 秒等待的红灯；现在读取前拒绝已声明超限的 `Content-Length`，读取中逐块累计实际字节数并在超过 4 MiB 时中止。两例均快速、脱敏地失败关闭，正常 Codex fixture 和既有末次绑定漂移检查不变；Codex 专项 Node 32/32。

本项只约束私有 receipt 验收器的 Account/Provider/Share 等控制面响应，不改生产 Codex HTTP/WS/image wire、模型 entitlement、receipt schema v1/revision 1 或控制面 API 的跨请求原子性。全量 audit Node 334/334、smoke Node 13/13；Codex reference-delta `--check-sources`、Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法和 `git diff --check` 均通过。`gpt_6_astra`、`gpt_image_2_5`、`gpt_image_2_5_flare`、`gpt_image_2_5_sunburst`、`ws_prewarm` 五个入口分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`；没有发送真实请求，也没有任何真实 receipt。新增 JS 逻辑仍在未提交工作树，既有参考提交态审计不等于新实现的 committed-target observation。Codex 此轮离线收口，下一类别为 Cursor；30 个独立真实验收项及最终整体 review 仍未完成，未执行 UI 自动化或部署测试。

### 18.18 Cursor 双 rail 私有 receipt 控制面响应大小栅栏

2026-10-08，Cursor 单类别接续。推荐 OmniRoute 的已提交 HEAD/tree 仍为 `443d66996d69` / `8d1c312f61d1`；22 项未提交 Codex 工作树改动继续排除，CUR-N2 的 no-new-wire 结论不变。目标自身 `scripts/smoke/cursor-real.mjs` 在共用 `requestJson` 中先用 `response.arrayBuffer()` 读完整个 Account/Provider/Share/目录响应，随后才检查 4 MiB。OAuth 的 `/api/accounts` 声明超限长度但不结束、API Key 的 `/api/providers` 无长度分块发送超限内容后不结束，两条本地回环反例都先等待约 5 秒超时。现在已声明超限的 `Content-Length` 在读取前拒绝，实际分块字节累计一旦超过 4 MiB 即中止；两例都约 0.2 秒脱敏失败，原有两 rail 正例、绑定漂移和 API Key 目录新鲜度用例保持通过。Cursor 专项 Node 36/36。

这是私有证据验收器的资源边界，不改生产 Cursor protobuf/session/目录发布 wire、OAuth/API Key 路由、receipt schema v2/revision 2 或跨控制面 API 原子性。全量 audit Node 337/337、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix 通过。两条 rail 各自以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有发真实请求，真实 receipt 仍各为 `null/live_pending`。此 JS 加固还在未提交工作树，既有参考提交态审计不能充作它的 committed-target observation。下一类别为 Grok；30 个独立真实验收项与最终整体 review 仍未完成，未执行 UI 自动化或部署测试。

### 18.19 Grok 三项私有 receipt 控制面响应大小栅栏

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a96`、`sub2api@ab99d56e` 的已提交 HEAD 不变；后者六项未提交 Codex 文件排除，GR-N4 的版本/header 决策仍保持 `live_pending_no_wire_change`。目标自身 `scripts/smoke/grok-real-receipt.mjs` 的共用 `requestJson` 原先等待整份响应后才检查 4 MiB。inference 的 `/api/accounts` 声明超限长度但不结束、media 的 `/api/providers` 无长度分块发送超限内容后不结束，两条本地回环反例先各自等待约 5 秒超时。现在读取前拒绝已声明超限的 `Content-Length`，读取中逐块累计实际字节，超过 4 MiB 立即中止；两例均约 0.2 秒脱敏失败。三个 operation 的正常 fixture、目录新鲜度及末次身份漂移用例保持通过，Grok 专项 Node 37/37。

本项只修私有证据验收器的资源边界，不改生产 Grok `0.2.111` 身份、group header、推理/media/WS wire、目录发布、receipt schema v1/revision 1 或 remote compaction 的 disabled 状态。全量 audit Node 340/340、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。inference、media、remote_compaction 分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有发送真实请求；三份真实 receipt 和 GR-N4 同账号版本/header 证据仍缺。新增 JS 逻辑在未提交工作树，不能以旧提交态审计充作 committed-target observation。下一类别为 Kiro；30 个独立真实验收项及最终整体 review 仍未完成，未执行 UI 自动化或部署测试。

### 18.20 Kiro 八路私有 receipt 控制面响应大小栅栏

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4` / tree `5d22f040` 的已提交状态不变。目标自身 `scripts/smoke/kiro-real-receipt.mjs` 在共用 `requestJson` 中先等待完整 Account/Provider/Share/两份目录响应，再检查 4 MiB。Builder ID / `us-east-1` 的 `/api/accounts` 声明超限长度但不结束、API Key / `eu-central-1` 的 `/api/providers` 无长度分块发送超限内容后不结束，两条本地回环反例先各等待约 5 秒超时。现在读取前拒绝已声明超限的 `Content-Length`，读取中逐块累计实际字节，超过 4 MiB 即中止；两例均约 0.2 秒脱敏失败。八路正常 fixture、目录时效与绑定漂移用例不变，Kiro 专项 Node 67/67。

本项只修私有证据验收器的资源边界，不改生产 Kiro/Amazon Q EventStream、KI-N5 usage authority、auth kind/region 路由、receipt schema v1/revision 1；Compact 与 shared cache 仍 disabled。全量 audit Node 343/343、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。四种 auth kind × 两 region 的八个入口逐一以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`；没有真实请求，八份 receipt 仍各为 `null/live_pending`。此 JS 加固未提交，既有参考提交态审计不能代替当前实现的 committed-target observation。下一类别为 Qoder；30 个独立真实验收项及最终整体 review 仍未完成，未执行 UI 自动化或部署测试。

### 18.21 Qoder 三 rail 私有 receipt 声明长度栅栏

2026-10-08，Qoder 单类别接续。`TokenRouter@6d676f1d` 仍要求本会话不可用的 `project-doc` 技能；未读取其业务源码，也未运行访问外仓的 `--check-sources`。本项只针对本仓库 `scripts/smoke/qoder-real.mjs`：原 `readLimited` 已逐块限制 8 MiB，但若控制面声明超限 `Content-Length`、只发送少量内容且不结束，三条 rail 都会等约 5 秒超时，不能在头部已知超限时及时失败。Global OAuth、Global PAT、CN OAuth 三个 loopback 反例先红后绿；现在读取前检查声明长度并取消响应 body，三例均约 0.2 秒失败。另有 Global OAuth 的无长度分块实际超限回归，确认逐块达到上限后取消 reader；正常三 rail、多 Surface 工具/终态和末次身份绑定 fixture 保持通过。Qoder 专项 Node 36/36。

本项只提高私有验收器的资源边界，不改变生产 COSY wire、Account/Provider/Share 选择、receipt schema v2/revision 2 或三 rail 状态。全量 audit Node 348/348、smoke Node 13/13；Qoder CLI oracle、自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix 通过。三个 `RUN_REAL=0` 入口分别为 `blocked_inputs/live_pending`，没有真实请求或 receipt。默认 delta 审计不能证明最新 TokenRouter 业务增量已审，仍为 `unreviewed_skill_gate`；本工作树 JS 改动也尚未形成 committed-target observation。下一类别为 CodeBuddy；30 个独立真实验收项及最终整体 review 仍未完成，未执行 UI 自动化或部署测试。

### 18.22 CodeBuddy 双站私有 receipt 控制面响应大小栅栏

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521` / tree `876656d0` 的已提交状态不变，两个未跟踪 `proxy.html`、`proxy.md` 继续排除。目标自身 `scripts/smoke/codebuddy-real-receipt.mjs` 在共用 `requestJson` 中先完整读取 Account/Provider/Share/三份目录响应，再检查 4 MiB。Intl 的 `/api/accounts` 声明超限长度但不结束、CN 的 `/api/providers` 无长度分块发送超限内容后不结束，两条本地回环反例先各等待约 5 秒超时。现在读取前拒绝已声明超限的 `Content-Length`，读取中逐块累计实际字节，超过 4 MiB 即中止；两例均约 0.2 秒脱敏失败。双站正常 fixture、末次绑定与目录 fresh TTL 用例保持通过，CodeBuddy 专项 Node 23/23。

本项只修私有证据验收器的资源边界，不改生产 CodeBuddy 转发、缓存 TTL、Intl/CN 站点绑定、receipt schema v2/revision 2；CB-N4 `deepseek-v4.1-flash` 仍 disabled。全量 audit Node 351/351、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。Intl/CN 各自以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，两站真实 receipt 与 CB-N4 厂商目录/接受性证据仍缺。此 JS 加固未提交，旧参考提交态审计不能代替新的 committed-target observation。八类本轮接续均有离线进展，但 30 个独立真实验收项及最终整体 review 仍未完成；未执行 UI 自动化或部署测试。

### 18.23 Antigravity 双 rail 私有 receipt 控制面响应大小栅栏

2026-10-08，新一轮从 Antigravity 单类别开始。推荐 `CLIProxyAPI@0f96f568`、`Antigravity-Manager@b601f5e7` 的已提交状态不变；AG-N12 并发目录代际/CAS 和 AG-D3 枚举差分已有本地先红后绿或明确不采纳记录，但仍缺真实 rail 与提交态证据。本项独立修目标 `scripts/smoke/antigravity-real.mjs`：其共用 `requestJson` 原先先完整读取 Account/Provider/Share/目录响应，随后才检查 4 MiB。`antigravity_oauth` 的 `/api/accounts` 声明超限长度但不结束、`agy_oauth` 的 `/api/providers` 无长度分块发送超限内容后不结束，两条本地 loopback 反例先各等待约 5 秒超时。现在读取前拒绝已声明超限的 `Content-Length`，读取中逐块累计实际字节，超过 4 MiB 即中止；两例均约 0.2 秒脱敏失败，原有私有路径、目录内容和末次绑定 fixture 保持通过。Antigravity 专项 Node 32/32。

本项只强化私有证据验收器的资源边界，不改生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 决策或 receipt schema v2/revision 2。全量 audit Node 354/354、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。两条 rail 分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有真实请求或 receipt。新增 JS 逻辑仍未提交，旧提交态审计不能充作它的 committed-target observation；30 个独立真实验收项和最终整体 review 仍未完成，未执行 UI 自动化或部署测试。

### 18.24 Claude 五项私有 receipt 控制面响应大小栅栏

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568` 的已提交 HEAD/tree 不变；CL-N7 续线程 alias 与 CL-N8 `pause_turn` 已有本地离线实现，但真实 thread/beta 接受性及五个 operation receipt 仍缺。本项独立修 `scripts/smoke/claude-real-receipt.mjs`：旧共用 `requestJson` 在完整读取 Account/Provider/Share/目录及 plan projection 后才检查 4 MiB。`oauth_inference` 的 `/api/accounts` 声明超限长度但不结束、`opus_5_5` 的 `/api/providers` 无长度分块发送超限内容后不结束，两条 loopback 反例先各等约 5 秒超时。现读取前拒绝已声明超限的 `Content-Length`，读取中逐块累计实际字节，超过 4 MiB 即中止；两例均约 0.2 秒脱敏失败。五项 operation 的正常、私有路径与末次绑定 fixture 保持通过，Claude 专项 Node 33/33。

本项只修私有验收器的资源边界，不改生产 Claude OAuth wire、CL-N7/CL-N8 语义、plan projection 或 receipt schema v1/revision 1。全量 audit Node 357/357、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix 通过。五个 operation 分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有真实请求或 receipt。新增 JS 逻辑未提交，不能以旧参考审计充作 committed-target observation；30 个独立真实验收项及最终整体 review 仍未完成，未执行 UI 自动化或部署测试。

### 18.25 Codex 私有 receipt 静态硬链接边界

2026-10-08，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568`、`codex2api@b1bfe1b4` 的已提交状态不变。本仓库 Codex 私有 receipt 已用真实路径拒绝仓库外符号链接指向仓库内；但同一文件在仓库内外建立硬链接时，外部路径的 `realpath` 仍在仓库外，旧验收器会接受它。`gpt_6_astra` 的合法 fixture 从仓库内文件硬链接到仓库外，先错误写出 `contract_verified/live_pending`；现在对已解析文件要求 `nlink=1`，在任何控制面请求前拒绝多硬链接文件。五个 operation 正常 fixture、控制面响应上限与末次绑定用例保持通过，Codex 专项 Node 33/33。

这只加固静态私有证据文件边界，不改变生产 Codex HTTP/WS/image wire、receipt schema v1/revision 1 或五个 operation 的真实状态。它不声称解决本地文件被并发替换的竞争，也不证明部署服务的 commit 与本地 HEAD 一致。全量 audit Node 358/358、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。五个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，没有真实请求或 receipt。当前 JS 加固未提交，旧参考审计不能充作新实现的 committed-target observation；30 个独立真实验收项及最终整体 review 仍未完成，未执行 UI 自动化或部署测试。

### 18.26 Cursor 双 rail 私有 receipt 静态硬链接边界

2026-10-08，Cursor 单类别接续。推荐 OmniRoute 已提交 HEAD/tree 仍为 `443d66996d69` / `8d1c312f61d1`，22 项未提交 Codex 文件继续排除；CUR-N2 的 no-new-wire 结论不变。目标双 rail 私有 receipt 已检查仓库外路径和 `realpath`，但外部路径若与仓库内文件为硬链接，旧验收器仍把同一 inode 当作仓库外证据接受。OAuth/API Key 两个合法 fixture 各自从仓库内硬链接到仓库外，先错误写出 `contract_verified/live_pending`；现在要求已解析文件 `nlink=1`，两例均在任何控制面请求前脱敏拒绝。双 rail 正常 fixture、响应上限、绑定漂移和 API Key 目录新鲜度检查保持通过，Cursor 专项 Node 39/39。

本项仅收紧静态私有文件边界，不改生产 Cursor protobuf/session/目录发布 wire、OAuth/API Key 身份或 receipt schema v2/revision 2。它不证明并发替换文件的原子防护，也不证明部署 commit 与本地 HEAD 相同。全量 audit Node 361/361、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。两条 rail 分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，真实 receipt 仍各自缺失。新增 JS 逻辑未提交，不能用旧参考审计充作 committed-target observation；30 个独立真实验收项和最终整体 review 仍未完成，未执行 UI 自动化或部署测试。

### 18.27 Grok 三项私有 receipt 静态硬链接边界

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a96`、`sub2api@ab99d56e` 的已提交状态不变，后者六项未提交 Codex 文件仍排除；GR-N4 同账号版本/header 决策仍缺真实证据。目标三项私有 receipt 已检查仓库外路径及符号链接的真实位置，但仓库内合法 fixture 与仓库外路径若是同一 inode 的硬链接，旧验收器仍会接受外部路径。inference、media、remote_compaction 三项分别用仓库内文件到仓库外的硬链接先复现错误写出 `contract_verified/live_pending`；现在对已解析文件要求 `nlink=1`，三例均在任何控制面请求前脱敏拒绝。正常 fixture、目录时效、末次身份绑定及响应大小检查保持通过，Grok 专项 Node 41/41。

本项只收紧静态私有证据文件边界，不改生产 Grok `0.2.111` 身份、group header、推理/media/WS wire、目录发布、receipt schema v1/revision 1 或 remote compaction disabled 状态。文件检查不提供并发替换的原子防护，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 365/365、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。三项分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有真实请求或 receipt；GR-N4 同账号版本/header 证据仍缺。本工作树的 JS 加固未提交，不能把旧提交态审计当作新实现的 committed-target observation。下一类别为 Kiro；30 个独立真实验收项与最终整体 review 仍未完成，未做 UI 自动化或部署测试。

### 18.28 Kiro 八路私有 receipt 静态硬链接边界

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4` / tree `5d22f040` 的已提交状态不变，KI-N5 context usage 仍只保留 shadow 对照。目标八路私有 receipt 已检查外部路径和符号链接的真实位置，但仓库内文件与仓库外 receipt 路径共用 inode 时，旧验收器仍会接受。Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 八个合法 fixture 各自从仓库内硬链接到仓库外，先全部错误写出 `contract_verified/live_pending`；现在要求已解析文件 `nlink=1`，八例均在任何控制面请求前以固定脱敏错误拒绝。八路正常 fixture、目录时效、末次绑定及响应大小用例保持通过，Kiro 专项 Node 76/76。

这只收紧静态私有文件边界，不改变生产 Kiro/Amazon Q EventStream、auth kind/region 路由、KI-N5 usage authority 或 receipt schema v1/revision 1；不能防止文件并发替换，也不能证明部署 commit 与本地 HEAD 一致。全量 audit Node 374/374、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法和差分检查通过。八个入口逐一以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有真实请求或 receipt；KI-N3 Compact、shared cache 继续 disabled。当前 JS 加固未提交，不能以旧提交态审计充作新实现的 committed-target observation；30 个独立真实验收项和最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一类别为 Qoder。

### 18.29 Qoder 三 rail 私有 receipt 输出目录真实位置栅栏

2026-10-08，Qoder 单类别接续。推荐 TokenRouter 最新业务源码仍受本会话缺失的 `project-doc` 技能门禁约束；本轮未读取该仓业务对象，也未运行访问外仓的 `--check-sources`，最新增量保持 `unreviewed_skill_gate`。目标自身 `scripts/smoke/qoder-real.mjs` 负责**写出** receipt，原先仅按字面路径要求输出位于仓库外；仓库外目录若是指向仓库内的符号链接，Global OAuth、Global PAT、CN OAuth 三个合法 fixture 均先错误写入仓库内并报告 `contract_verified/live_pending`。现在在任何控制面请求前解析输出父目录真实位置并拒绝仓库内目标，写入前再复核一次；三例均不发请求、不产生文件。正常三 rail fixture、末次绑定、工具参数、终态和响应上限检查保持通过，Qoder 专项 Node 40/40。

这是私有证据输出位置的静态防护，不是对并发目录替换的原子保证；也不改变生产 COSY wire、三 rail 的 Account/Provider/Share 选择或 receipt schema v2/revision 2。全量 audit Node 378/378、smoke Node 13/13；Qoder CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix 通过。三个 `RUN_REAL=0` 入口分别仍为 `blocked_inputs/live_pending`，没有真实请求或 receipt。默认 delta 审计不能代替最新 TokenRouter 源码补审；本工作树 JS 加固未提交，也不能用旧提交态证据冒充新实现的 committed-target observation。30 个独立真实验收项与最终整体 review 仍未完成，未做 UI 自动化或部署测试；下一类别为 CodeBuddy。

### 18.30 CodeBuddy 双站私有 receipt 静态硬链接边界

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521` / tree `876656d0` 的已提交状态不变，未跟踪 `proxy.html`、`proxy.md` 继续排除；CB-N4 仍缺 CN 厂商目录/接受性证据。目标双站私有 receipt 已检查仓库外路径和符号链接的真实位置，但仓库内文件与仓库外路径是硬链接时，旧验收器仍会接受。Intl/CN 两个合法 fixture 各自从仓库内硬链接到仓库外，先错误写出 `contract_verified/live_pending`；现要求已解析 receipt 文件 `nlink=1`，两例均在任何控制面请求前以固定脱敏错误拒绝。双站正常 fixture、目录 fresh TTL、末次绑定和响应大小检查保持通过，CodeBuddy 专项 Node 26/26。

本项仅收紧静态私有证据文件边界，不改变生产 CodeBuddy 转发、缓存 TTL、Intl/CN 站点绑定或 receipt schema v2/revision 2；静态检查不防止并发文件替换，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 381/381、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。Intl/CN 各自以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有真实请求或 receipt；CB-N4 `deepseek-v4.1-flash` 仍 disabled。当前 JS 加固未提交，不能把旧提交态审计当成新实现的 committed-target observation；30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一轮从 Antigravity 单类别继续。

### 18.31 Antigravity 双 rail 私有 receipt 静态硬链接边界

2026-10-08，Antigravity 单类别接续。推荐 `CLIProxyAPI@0f96f568`、`Antigravity-Manager@b601f5e7` 的已提交状态不变。目标私有 receipt 已按真实路径拒绝仓库外符号链接指向仓库内，但仓库内合法文件与仓库外路径共用 inode 时，旧验收器仍会接受。`antigravity_oauth`、`agy_oauth` 两条 rail 分别将仓库内 fixture 硬链接到仓库外，先错误写出 `contract_verified/live_pending`；现要求 receipt 文件 `nlink=1`，两例都在任何控制面请求前以固定脱敏错误拒绝。双 rail 正常 fixture、目录内容和时间窗口、末次绑定、响应大小用例保持通过，Antigravity 专项 Node 35/35。

本项只加固静态私有证据文件边界，不改生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 决策或 receipt schema v2/revision 2；它不防止文件并发替换，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 384/384、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。两条 rail 分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有真实请求或 receipt；AG-N6 和 compaction 仍缺独立真实证据。当前 JS 加固未提交，不能把旧提交态审计当作新实现的 committed-target observation；30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一类别为 Claude。

### 18.32 Claude 五项私有 receipt 静态硬链接边界

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568` / tree `f43e8e15` 的已提交状态不变；CL-N7 续线程别名和 CL-N8 `pause_turn` 已有本地离线实现，但真实 thread/beta 接受性仍缺。目标五项私有 receipt 已按真实路径拒绝仓库外符号链接指向仓库内；若仓库内文件和仓库外路径是硬链接，旧验收器仍会接受。`oauth_inference`、`max_5x_plan`、`max_20x_plan`、`fable_5_1`、`opus_5_5` 五个合法 fixture 各自从仓库内硬链接到仓库外，先全部错误写出 `contract_verified/live_pending`；现要求 receipt 文件 `nlink=1`，五例均在控制面请求前以固定脱敏错误拒绝。正常 fixture、套餐投影、绑定漂移和响应大小用例保持通过，Claude 专项 Node 39/39。

本项仅加固静态私有文件边界，不改生产 Claude OAuth wire、CL-N7/CL-N8 语义、plan projection 或 receipt schema v1/revision 1；不防止并发文件替换，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 390/390、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix 通过。五项分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有真实请求或 receipt。当前 JS 加固未提交，不能以旧提交态审计充作新实现的 committed-target observation；30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一类别为 Codex。

### 18.33 Codex 私有 receipt 请求期间替换栅栏

2026-10-08，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568`、`codex2api@b1bfe1b4` 的提交态不变；五个真实 operation 仍缺独立 receipt。本仓库验收器原先在请求 Account/Provider/Share 和可选模型目录之后才按路径读取 receipt；即使启动时通过路径、权限和硬链接检查，期间将无效 receipt 原子替换为同路径有效文件，旧流程也会接受后者。`gpt_6_astra`、三个 Image 2.5 exact model、`ws_prewarm` 五项各自用首个控制面请求触发替换，先全部错误写出 `contract_verified/live_pending`。现在请求前以只读文件描述符取得最多 1 MiB 的快照，并复核文件类型、dev/inode、链接数、大小和权限；后续只解析此快照。五例均拒绝请求期间出现的替代文件，正常 operation、静态硬链接、绑定漂移及响应大小用例保持通过，Codex 专项 Node 39/39。

这收紧的是私有证据读取窗口，不改生产 Codex HTTP/WS/image wire、模型 entitlement、receipt schema v1/revision 1 或跨控制面 API 原子性。它不证明文件在快照读取瞬间遭并发原位改写时也能获得原子视图，更不证明部署 commit 与本地 HEAD 一致。全量 audit Node 396/396、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。五个 `RUN_REAL=0` 入口仍各为 `blocked_inputs/live_pending`，没有真实请求或 receipt。新增 JS 逻辑未提交，旧参考审计不能充作新实现的 committed-target observation；30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一类别为 Cursor。

### 18.34 Cursor 双 rail 私有 receipt 请求期间替换栅栏

2026-10-08，Cursor 单类别接续。推荐 OmniRoute 已提交 HEAD/tree 仍为 `443d66996d69` / `8d1c312f61d1`，22 项未提交 Codex 工作树改动继续排除，CUR-N2 的 no-new-wire 结论不变。目标 OAuth/API Key 私有验收器原先启动时检查 receipt 路径、权限和硬链接，完成 Account/Provider/Share 与目录请求后才按路径读取；期间将无效 receipt 原子替换为有效文件，旧流程仍会接受。OAuth 首个 Account 请求、API Key 首个 Provider 请求分别触发替换，两条合法 fixture 均先错误写出 `contract_verified/live_pending`。现在请求前从只读文件描述符限量获取最多 1 MiB 的快照，复核文件类型、dev/inode、链接数、大小和权限；后续只解析该快照。两例均拒绝替代文件，正常双 rail、API Key 目录时效、绑定漂移和响应大小检查保持通过，Cursor 专项 Node 42/42。

本项只收紧私有证据的读取窗口，不改生产 Cursor protobuf/session/目录发布 wire、两 rail 身份或 receipt schema v2/revision 2；不承诺快照读取瞬间的并发原位改写具有原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 399/399、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。两条 rail 分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`，没有真实请求或 receipt。新增 JS 逻辑未提交，不能把旧参考审计当作新实现的 committed-target observation；30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一类别为 Grok。

### 18.35 Grok 三项私有 receipt 请求期间替换栅栏

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态不变，后者六项未提交 Codex 文件继续排除。目标验收器启动时已检查 receipt 的仓库外路径、权限和链接数，却在控制面请求和目录获取后按路径重读；inference、media、remote_compaction 分别在首个 Account 请求将无效文件原子替换为有效文件，三项旧流程均错误写出 `contract_verified/live_pending`。现在请求前用只读描述符限量读取最多 1 MiB，复核文件类型、dev/inode、链接数、大小和权限；后续仅解析这个快照。三个替换反例均失败关闭，正常 operation、静态硬链接、目录时效、末次绑定和响应大小用例保持通过，Grok 专项 Node 45/45。

本项只收紧私有证据读取窗口，不改生产 Grok `0.2.111` 身份、group header、推理/media/WS wire、目录发布、receipt schema v1/revision 1 或 remote compaction disabled 状态；快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 403/403、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。三个 `RUN_REAL=0` 入口仍各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；GR-N4 同账号版本/header 真实证据仍缺。新增 JS 未提交，不能以旧提交态审计冒充新实现的 committed-target observation。30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试；下一类别为 Kiro。

### 18.36 Kiro 八路私有 receipt 请求期间替换栅栏

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` / tree `5d22f040f0e4b0aa775e61547540b3c040293cd8` 的已提交状态不变。目标八路私有验收器启动时已检查 receipt 的仓库外路径、权限和硬链接，却在 Account/Provider/Share 及双目录请求之后按路径重读；Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 分别在首个 Account 请求将无效文件原子替换为有效文件，旧流程八例均错误写出 `contract_verified/live_pending`。现在请求前用只读描述符限量取得最多 1 MiB 的快照，复核文件类型、dev/inode、链接数、大小和权限；后续只解析固定快照。八条替换反例均失败关闭，正常八路、静态硬链接、目录时效、末次绑定和响应大小用例保持通过，Kiro 专项 Node 85/85。

本项只收紧私有证据读取窗口，不改生产 Kiro/Amazon Q EventStream、auth kind/region 路由、KI-N5 usage authority 或 receipt schema v1/revision 1；Compact 与 shared cache 仍 disabled。快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 412/412、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。八个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，无真实请求或 receipt。新增 JS 未提交，不以旧 committed-target observation 冒充当前实现证据；30 个独立真实验收项、KI-N5 计量权威证据及最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一类别为 Qoder，但其最新 TokenRouter 业务源码仍受 `project-doc` 技能门禁约束。

### 18.37 Qoder 三 rail 私有 receipt 排他发布栅栏

2026-10-08，Qoder 单类别接续。TokenRouter 的 `project-doc` 技能在当前会话仍不可调用；未读取其业务源码或运行访问该仓的命令，最新增量保持 `unreviewed_skill_gate`。只检查目标自身 `scripts/smoke/qoder-real.mjs`：旧验收器先确认输出路径不存在，再写临时文件并用 `renameSync` 发布；若另一文件在检查后出现，重命名仍会覆盖。Global OAuth、Global PAT、CN OAuth 三条 rail 的本地预加载反例各自在临时文件写完后创建目标哨兵文件，旧流程均覆盖它并错误报告 `contract_verified/live_pending`。现以同目录硬链接排他发布，目标已存在时失败而不覆盖；仅清理本次实际创建的临时文件。三条正常 rail 的 receipt 仍为 `0600`，成功或冲突后均无残留临时文件；Qoder 专项 Node 44/44。

本项只收紧私有证据写出边界，不改生产 COSY wire、Account/Provider/Share 绑定、receipt schema v2/revision 2 或 rail/site 选择；父目录在真实路径检查与写入之间被并发替换的风险、跨 API 非原子性和部署 commit 一致性均不由此证明。全量 audit Node 416/416、smoke Node 13/13；Qoder CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。三条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，无真实请求或 receipt；默认审计不能替代受门禁的最新 TokenRouter 源码补审。新增 JS 未提交，不能以旧 committed-target observation 冒充当前实现证据；30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一类别为 CodeBuddy。

### 18.38 CodeBuddy 双站私有 receipt 请求期间替换栅栏

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` / tree `876656d0780afde26af9b1ab0a87e0ea58d8c752` 的提交态未变，未跟踪 `proxy.html`、`proxy.md` 继续排除。目标 Intl/CN 验收器启动时已检查 receipt 的仓库外路径、权限和硬链接，却在 Account/Provider/Share 与三份目录请求后按路径重读；两站各自于首个 Account 请求将无效文件原子替换为有效文件，旧流程均错误写出 `contract_verified/live_pending`。现在请求前用只读描述符限量获取最多 1 MiB 快照，复核文件类型、dev/inode、链接数、大小和权限；后续只解析固定快照。两条替换反例均失败关闭，正常双站、静态硬链接、目录新鲜度、末次绑定和响应大小用例保持通过，CodeBuddy 专项 Node 29/29。

本项只收紧私有证据读取窗口，不改生产 CodeBuddy 转发、Intl/CN 站点绑定、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled 决策；快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 419/419、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。Intl/CN `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，无真实请求或 receipt；CB-N4 仍缺 CN 厂商目录/接受性证据。新增 JS 未提交，不能以旧 committed-target observation 冒充当前实现证据。30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试；下一步按计划进行跨类别核查，不将这轮 fixture 当成总体完成。

### 18.39 Antigravity 双 rail 私有 receipt 请求前限量快照

2026-10-08，按单类别循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的提交态未变。目标双 rail 私有验收器虽检查 receipt 仓库外真实路径、权限及单链接，却在 Account/Provider/Share 和目录请求后才按路径读取，且没有 receipt 大小上限。`antigravity_oauth`、`agy_oauth` 各自在首个 Account 请求将无效文件原子替换为有效文件，旧流程均错误写出 `contract_verified/live_pending`；两份超过 1 MiB 的文件也先触发控制面请求。现在请求前从只读文件描述符限量获取最多 1 MiB 快照，复核文件类型、dev/inode、链接数、大小和权限；后续只解析固定快照。四条反例均失败关闭，正常双 rail、静态硬链接、目录内容/时效、末次绑定和响应大小用例保持通过，Antigravity 专项 Node 41/41。

本项只收紧私有证据读取边界，不改生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 决策或 receipt schema v2/revision 2；快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 425/425、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。两条 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，无真实请求或 receipt；AG-N6 和 compaction 仍缺独立真实证据。新增 JS 未提交，不能以旧 committed-target observation 冒充当前实现证据。30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试；下一类别为 Claude。

### 18.40 Claude 五项私有 receipt 请求前限量快照

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` / tree `f43e8e15ea31e3d1dd1cf83250d9fdb627c06525` 的提交态未变。目标五项私有验收器虽检查 receipt 仓库外真实路径、权限与单链接，却在 Account/Provider/Share、模型及 plan 请求后才按路径读取，且没有 receipt 大小上限。`oauth_inference`、`max_5x_plan`、`max_20x_plan`、`fable_5_1`、`opus_5_5` 各自在首个 Account 请求将无效文件原子替换为有效文件，旧流程全部错误写出 `contract_verified/live_pending`；五份超过 1 MiB 的文件也先触发控制面请求。现在请求前用只读描述符限量取得最多 1 MiB 快照，复核文件类型、dev/inode、链接数、大小和权限；后续只解析固定快照。十条反例均失败关闭，正常五项、静态硬链接、plan projection、末次绑定和响应大小用例保持通过，Claude 专项 Node 51/51。

本项只收紧私有证据读取边界，不改生产 Claude OAuth wire、CL-N7/CL-N8 语义、plan projection 或 receipt schema v1/revision 1；快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 437/437、smoke Node 13/13；Claude reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，无真实请求或 receipt；续线程及 `pause_turn` 的厂商接受性仍缺独立证据。新增 JS 未提交，不能以旧 committed-target observation 冒充当前实现证据。30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试；下一类别为 Codex。

### 18.41 Codex Astra 与 WS prewarm 账号 manifest 验收栅栏

2026-10-08，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的提交态未变。复核发现目标普通 `/v1/models` 列表可包含 `gpt-6-astra` 内置冷启动兼容项，不能独自证明绑定账号 manifest 的授权；旧 `gpt_6_astra` 与 `ws_prewarm` 私有验收器却仅核对普通列表模型 ID。两项“普通列表有、账号 manifest 无”的回环 fixture 均先错误写出 `contract_verified/live_pending`。现额外经同一 Share 的允许路径 `/v1/models?client_version=0.144.1` 读取 Server 转发的账号 manifest，要求 `models[*].slug` 有且仅有一个精确模型。缺失/重复四个反例均失败关闭，普通目录、完整 receipt 和末次绑定检查仍保留；三个 Image 2.5 操作不增加 manifest 请求。

固定查询版本与当前 `src/codex_identity.rs::DEFAULT_CODEX_VERSION` 一致并有测试锚定，不证明运行中动态同步版本或真实 entitlement；Router Share 不放行 `/backend-api/codex/models`，故只使用既有 `/v1/models` 查询分派，不改 Router、生产 Codex HTTP/WS/image wire、模型 entitlement 规则或 receipt schema v1/revision 1。Codex 专项 Node 45/45、全量 audit Node 443/443、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，无真实请求或 receipt；新增 JS 未提交，不能以旧 committed-target observation 冒充当前实现证据。30 个独立真实验收项及最终整体 review 仍未完成，未做 UI 自动化或部署测试；下一类别为 Cursor。

### 18.42 Cursor OAuth 目录权威边界复核

2026-10-08，Cursor 单类别接续。推荐的 `OmniRoute@443d66996d69` / tree `8d1c312f61d1` 已提交对象未变；其 22 项工作树内容继续排除。对目标 `src/api/models.rs::openai_model_list`、`src/proxy/cursor/model.rs::resolve_cursor_model_with_catalog`、`src/api/mod.rs::proxy_models_for_selection` 与双 rail 私有验收器逐项核对：OAuth 的 `/v1/models` 来自 Provider 配置及内置 Cursor 别名，未带账号动态目录来源/时间戳；`*-fast` 可作为请求 selector，但列表命中本身不证明该 OAuth 账号具备厂商 entitlement。API Key 则通过精确凭据作用域的 `cursor_public_api` 动态目录发布，并已有 source、stale 与 5 分钟新鲜度验收。两 rail 的目录权威不同，不能把 API Key 的 metadata 规则直接套到 OAuth，也不能把 OAuth 静态目录升级为真实授权证据。

现有 OAuth 验收仍要求精确 Account/Provider/Share 代际、请求前私有 receipt 快照、22 项检查、10 份 body hash、5 项测量和末次绑定复核；这些都是离线验收结构门槛，最终账号接受性仍须仓库外真实 receipt。未发现新的 committed Cursor wire 或可复现生产缺口，故不制造代码改动。Cursor 专项 Node 42/42，reference-delta `--check-sources` 通过；两条 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，且当前环境没有真实验收输入。CUR-N1 OAuth/API Key 继续分别 `receipt=null/live_pending`，最终整体 review 仍待 30 项真实证据和专项门槛；未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.43 Grok 账号自定义头的身份命名空间栅栏

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 的已提交实现仍以 `x-grok-client-mode`、`x-grok-session-id`、`x-grok-req-id`、`x-grok-agent-id` 等 `x-grok-*` 头参与自身请求身份；`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 无新提交，其六项未提交 Codex 文件排除。目标此前只逐个拒绝少量 Grok 账号 `extraHeaders`，虽然已封锁 `x-grok-conv-group-id`，仍允许账号手工注入 `X-Grok-Client-Mode: headless` 等未审查身份头。实际账号头合并的定点测试先复现错误接受，再将精确 `grok_oauth` 的整个 `x-grok-*` 命名空间交给代理控制；原有 `x-xai-token-auth`、`x-authenticateresponse` 禁止项不变，非 Grok Provider 不受此分支影响。正常的 `x-enterprise-sso` 仍允许，默认生产请求不新增任何 Grok 头。

修复只阻断账号配置绕过 GR-N4 header 门槛，不证明厂商接受 `1.0.40`/其他版本或 group header，也不改 `0.2.111`、请求体、目录、HTTP/WS/media wire、receipt schema 与 remote compaction disabled 决策。Grok Rust 231/231、全量 Rust library 3284 passed / 1 ignored，其他目标 124 + 1 + 1 passed；Grok receipt Node 45/45、Clippy `--all-targets -D warnings`、rustfmt、Grok reference-delta `--check-sources`、Provider Phase 0/contract/coverage/UI matrix 均通过。三项 `RUN_REAL=0` 仍分别为 `blocked_inputs/live_pending`；GR-N4 同账号 catalog/HTTP/WS 版本及 header 接受性与三项 operation 私有 receipt 继续缺失。当前实现未提交，不能伪造 committed-target observation；最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.44 Kiro 八路真实 receipt 的认证方式与运行区域绑定

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` / tree `5d22f040f0e4b0aa775e61547540b3c040293cd8` 的已提交状态未变。目标八份 receipt 虽在文件及 scope digest 中写入所选 `authKind`/`region`，公开 Account 视图却只给 token 存在性和 `tokenType`；旧验收器无法验证 Builder ID/IdC/Social 真实分流，也不能确认账号实际运行区域。两条回环反例把同一 Account 投影改为别的 auth kind 或 region，旧流程均错误接受。现由 Server 控制面只公开 Kiro 专属 `kiroCredentialScope.authKind/runtimeRegion`：认证方式复用 Kiro refresh dispatch 的解析，区域复用生产出站的 operational runtime identity；不暴露 profile ARN、raw profile、token 或机器身份。八路验收要求两个字段与选定 scope 严格相等，缺字段、无效运行身份及任何不匹配均失败关闭；文件 receipt schema v1/revision 1 不变。

KI-N5 的 context usage 扣减仍只做 shadow，不因该作用域投影改变生产计量；Kiro/Amazon Q Compact 与 shared cache 仍 disabled。两条反例先红后绿，另有旧服务缺字段的 fail-closed 用例；八路正常 fixture、目录时效及代际漂移保持通过。Kiro receipt Node 89/89、全量 Node audit 447/447、Kiro Rust 190/190、Account 投影与 auth-kind 定点 Rust 2/2；全量 Rust library 3286 passed / 1 ignored，其他目标 124 + 1 + 1 passed。Clippy `--all-targets -D warnings`、rustfmt、Kiro reference-delta `--check-sources`、Provider Phase 0/Server contract/coverage/UI matrix、文档索引及差分空白检查均通过。八个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，真实 Account/region receipt 仍分别缺失，不能以 fixture、控制面投影或参考源码冒充厂商接受性。当前增量未提交，不写假的 committed-target observation；未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 源码仍受 `project-doc` 技能门槛约束。

### 18.45 Qoder 三 rail 真实 receipt 的站点与凭据类型绑定

2026-10-08，Qoder 单类别接续。推荐 TokenRouter 最新业务源码仍因当前会话无法调用其要求的 `project-doc` 技能而未读；没有运行访问该仓的 `--check-sources`，最新增量保持 `unreviewed_skill_gate`。目标三 rail 验收器原先仅凭 `qoder-{site}-` Account ID 前缀和 token 存在性判断站点/凭据类型，不能证明生产 profile 的 `site`、`credentialRail`。Global OAuth、Global PAT、CN OAuth 的回环 fixture 分别构造错 rail、缺投影、错站点，旧验收器都错误写出 `contract_verified/live_pending` receipt。现由 Server 仅对通过 `QoderAccountProfile::parse` 验证的 Qoder Account 发布脱敏 `qoderCredentialScope.site/credentialRail`；验收器要求与所选 rail/site 精确相等，旧服务缺字段也失败关闭，且在数据面请求前拒绝。投影测试确认不泄露 UID、machine ID 或 PAT；三条反例先红后绿。

本项不改变 COSY wire、账号路由/凭据恢复、receipt schema v2/revision 2 或任何真实 rail 状态。Qoder receipt Node 48/48、Qoder Rust 76/76、全量 audit Node 451/451、smoke Node 13/13；全量 Rust 在默认测试线程栈下于一条 CodeBuddy 测试发生 stack overflow，以 `RUST_MIN_STACK=16777216` 重跑为 library 3287 passed / 1 ignored，其他目标 124 + 1 + 1 passed。Clippy `--all-targets -D warnings`、rustfmt、Qoder CLI oracle、默认自包含 reference-delta、Provider Phase 0/Server contract/coverage/UI matrix、文档索引、脚本语法及差分空白检查均通过。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，无真实请求或 receipt。控制面投影加 loopback fixture 不能证明登录、PAT exchange 或厂商接受性；Global OAuth、Global PAT、CN OAuth 的独立真实 receipt 仍缺。当前工作树未提交，不伪造 committed-target observation；未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.46 CodeBuddy 双站真实 receipt 的 Account profile 站点绑定

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` / tree `876656d0780afde26af9b1ab0a87e0ea58d8c752` 的提交态不变；未跟踪 `proxy.html`、`proxy.md` 排除。参考凭据以 domain 区分 CN/Global ChatBase，而目标生产 profile 把 site 与受控 domain 共同用于身份/端点选择。目标双站私有验收器此前只把脚本所选 `intl/cn` 写入 scope digest 和 receipt，却没有核对选定 Account 的 profile site。Intl/CN 各自的错站点、缺投影和验收末次读取站点漂移共六条回环反例先错误写出 `contract_verified/live_pending`；现由 Server 仅对通过 `CodeBuddyAccountProfile::parse` 验证的 Account 发布脱敏 `codebuddyCredentialScope.site`，validator 在首末两次 Account 读取都要求与所选站点精确相等，旧服务缺字段失败关闭。投影定点测试确认不泄露 domain、UID 或 token。

这是控制面证据门槛，不改变 CodeBuddy 生产请求、Intl/CN 站点路由、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled 决策。CodeBuddy receipt Node 36/36、关键词 Rust 72/72、全量 audit Node 458/458、smoke Node 13/13；`RUST_MIN_STACK=16777216 cargo test --all-targets --quiet` 为 library 3288 passed / 1 ignored，其他目标 124 + 1 + 1 passed。Clippy `--all-targets -D warnings`、rustfmt、CodeBuddy reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法及差分空白检查均通过。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，无真实请求或 receipt。两站仍须各自提供完整真实 receipt，fixture 不证明厂商接受性；当前工作树未提交，不能用旧 committed-target observation 充作新增实现证据。未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.47 Antigravity 双 rail 真实 receipt 的 OAuth refresh 凭据门槛

2026-10-08，按单类别循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 Antigravity OAuth token response 明确区分 access/refresh token；`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的既有提交态审计不变。目标真实验收含 `same_account_first_401_recovery`，旧 `scripts/smoke/antigravity-real.mjs::validateAccount` 却只要求 access token，缺 refresh token 的 Account 也能通过并宣称该恢复检查完成。`antigravity_oauth`、`agy_oauth` 各自初始缺 refresh token 或末次读取时丢失该凭据，共四个回环反例先错误接受 `contract_verified/live_pending`。现首末两次 Account 快照都要求 `hasRefreshToken===true`；初始缺失在目录请求前失败，末次丢失在 receipt 接受前失败。正常双 rail fixture、目录快照、Provider/Share/代际漂移检查均保持通过。

此项只收紧私有验收器的凭据存在性证据，不证明 refresh token 有效、401 重放得到厂商接受，也不改变生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 决策或 receipt schema v2/revision 2。Antigravity receipt Node 46/46、全量 audit Node 463/463、smoke Node 13/13；参考 delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。两条 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，没有真实请求或 receipt。当前 JS 增量未提交，不能用旧 committed-target observation 冒充新验收逻辑；30 个独立真实项和最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.48 Claude 四项恢复/套餐 receipt 的凭据能力门槛

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 Claude OAuth refresh 路径以独立 refresh token 发起交换；目标自身又明确区分 `refreshable_oauth` 与 `access_only_setup_token`，后者能做部分推理但不进行 token refresh，quota/profile 403 只降级 enrichment。旧五项私有验收器仅检查 access token：`oauth_inference` 即使要求 `same_account_first_401_recovery`，Max 5x/20x 与 Fable 5.1 即使要求 fresh plan，仍会接受 access-only、缺能力投影或缺 refresh token 的 Account。四项 × 初始 access-only、缺能力、缺 refresh、验收后能力漂移、验收后 refresh 凭据消失共 20 个回环反例先全部错误接受。现这四项在首末两次 Account 快照都要求 `credentialCapability=refreshable_oauth` 与 `hasRefreshToken=true`；Opus 5.5 没有这组恢复/套餐检查，access-only 正例仍通过，不误封其合法推理路径。

这只提高私有 receipt 的账号凭据拓扑门槛，不证明 token 实际可刷新、套餐真实权益或厂商接受 CL-N7/CL-N8，也不改变生产 Claude OAuth wire、plan projection、receipt schema v1/revision 1 和五项独立 live 状态。Claude receipt Node 73/73、全量 audit Node 485/485、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/coverage/UI matrix、文档索引、脚本语法、rustfmt、`cargo check --lib` 与差分空白检查均通过。五项 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求或 receipt。当前 JS 改动未提交，不能将旧 committed-target observation 记为新逻辑证据；真实 operation receipt 继续各自 `live_pending`。未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.49 Codex 真实 manifest 的签名 Share / Provider 作用域核对

2026-10-08，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的提交态未变。核查确认生产 `forward_codex_models_manifest` 已从 Router 签名 Share 解析固定 Provider/Account；此前疑点不是随机选号。真正的薄弱点在私有验收器：Astra 与 WS prewarm 的普通模型目录请求虽带 `providerId`，manifest 请求却只有 `client_version`。如果提供了另一 Share 的 Router token 且其账号也有同名模型，脚本不能证明 manifest 来自声明的 Share/Provider。两项“请求必须声明所选 Share/Provider”回环用例先红（旧请求缺参数）；现验收器在 manifest 查询中发送 `expectedShareId/expectedProviderId`，Server 选出签名 Share 的实际 Provider 后、上游请求前核对，不匹配即返回冲突。可选参数不转发上游，普通客户端保持原行为；receipt schema v1/revision 1 不变。

Codex receipt Node 48/48、全量 Node audit 488/488、smoke Node 13/13；Rust 全量 `RUST_MIN_STACK=16777216 cargo test --all-targets --quiet` 为 library 3289 passed / 1 ignored，其余目标 124 + 1 + 1 passed；`cargo check --lib`、Clippy `--all-targets -D warnings`、rustfmt、Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引及差分空白检查通过。此检查只证明测试样本中请求携带预期 ID，Server 对不匹配会拒绝；不证明 Router token 实际指向哪份真实 Share、部署 commit 或厂商 entitlement。五项真实 operation receipt 仍缺，不可由 fixture 冒充。当前工作树未提交，不能将旧 committed-target observation 当成本轮实现证据；30 个独立真实项和最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.50 Cursor 双 rail 模型目录的签名 Share / Provider 作用域核对

2026-10-08，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` / tree `8d1c312f61d115767c9478416cb039750eaf113a` 的已提交 Cursor wire 路径无新差异，22 项未提交 Codex 文件继续排除，CUR-N2 保持 `reviewed_no_wire_delta`。目标 `/v1/models` 已按 Router 签名 Share 选择 Provider，而旧双 rail 验收请求仅带 `app/providerId`，后者只是脚本声明，不参与服务端选择；若 Router token 指向另一 Share 且其目录有同名 `*-fast` ID，验收器无法证明所选绑定。OAuth、API Key 两条正常回环 fixture 加入预期 Share/Provider 参数断言后先红。现脚本发送 `expectedShareId/expectedProviderId`；服务端在校验签名 Share 并解析实际 Provider 后、执行目录工作前逐项比对，错误作用域返回冲突。签名 Router ingress 的端到端测试覆盖匹配返回与错 Share/错 Provider 两种拒绝；普通客户端不带参数时行为不变。

此门槛不会把 OAuth 的静态别名当作真实 entitlement；API Key 仍要求精确凭据作用域、`cursor_public_api` 来源和新鲜时限。它不更改 Cursor protobuf/session/目录发布 wire、两 rail 身份或 receipt schema v2/revision 2，也不证明 Router token、部署 commit 或厂商接受性。Cursor receipt Node 42/42、全量 Node audit 488/488、smoke Node 13/13；Rust 全量 `RUST_MIN_STACK=16777216 cargo test --all-targets --quiet` 为 library 3291 passed / 1 ignored，其余目标 124 + 1 + 1 passed；`cargo check --lib`、Clippy `--all-targets -D warnings`、rustfmt、Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引与差分空白检查通过。OAuth/API Key 的 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，两份真实 receipt 仍各自缺失。当前工作树未提交，不能将旧 committed-target observation 冒充本轮实现证据；30 个独立真实项和最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.51 Grok 三项真实目录验收的签名绑定核对

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态不变；后者六项未提交 Codex 文件排除。参考 sub2api 的 Grok model sync 显式按 Account 构造访问凭据；目标 Server 的 `/v1/models` 则按 Router 签名 Share 选择 Grok Provider/Account，不使用普通 `providerId` 查询来选择。旧 inference、media、remote_compaction 私有验收器只带 `app/providerId`，Router token 若属于另一 Share 且目录有同名模型，不能证明所选绑定。三项正常回环 fixture 加入预期 Share/Provider 参数断言后均先红；现脚本发送 `expectedShareId/expectedProviderId`，复用上一轮已通过签名 ingress 端到端测试的 Server 目录栅栏，在目录处理前拒绝错误 Share/Provider。receipt schema v1/revision 1 不变。

本项只加强三份私有目录证据，不将目录命中当作真实厂商 entitlement，也不更改 Grok 生产 HTTP/WS/media wire、默认 `0.2.111`、group header 或 remote compaction disabled 决策。GR-N4 仍须同一固定 Account 的 catalog/HTTP/WS 版本与 header 接受/拒绝记录，不能由此查询代替。Grok receipt Node 45/45、全量 Node audit 488/488、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、Rust 签名目录路由定点测试、rustfmt 与差分空白检查通过。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，无真实请求或 receipt。当前工作树未提交，不能补写虚构的 committed-target observation；30 个独立真实项和最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.52 Kiro 八路双目录的签名 Share / App Provider 验收绑定

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的提交态未变，原有 KI-N4/KI-N5 差异结论也不变。目标的八份 auth-kind×region 私有 receipt 各自要求同一 Share 下 Claude、Codex 两份 fresh catalog；服务端目录按 Router 签名 Share 与 App 选 Provider，旧验收器只在查询中写普通 `app/providerId`，并不能证明 token 指向 receipt 所选 Share。八路各自的 Claude/Codex 请求缺少预期绑定参数，独立回环子测试先全部红。现每条目录请求带 `expectedShareId` 与该 App 的 `expectedProviderId`，复用已由签名 Router ingress 端到端测试证明的 Server 目录栅栏；测试另外要求同一 scope 确实各请求一次 Claude 和 Codex，不允许同一 App 双次冒充双目录。普通客户端无需这些参数，receipt schema v1/revision 1 不变。

本项只是私有目录证据门槛，不把 `kiroCredentialScope` 投影、目录模型存在或参考源码当作真实厂商接受性；KI-N5 context usage 继续 shadow，Kiro/Amazon Q Compact 与 shared cache 继续 disabled。Kiro receipt Node 97/97、全量 Node audit 496/496、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、签名目录路由 Rust 定点测试、rustfmt 与差分空白检查均通过。本轮只改 JS 与文档，未重跑无变化的全量 Rust 套件。八个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，八份真实 receipt 仍独立缺失。当前工作树未提交，不补伪 committed-target observation；30 个独立真实项和最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 业务源码仍受 `project-doc` 技能门槛约束。

### 18.53 Qoder 三 rail 三 Surface 目录的签名绑定核对

2026-10-08，Qoder 单类别接续。推荐 TokenRouter 最新业务源码依旧要求本会话不可用的 `project-doc` 技能；没有读取该仓业务对象，也没有运行访问外仓的 `--check-sources`，最新增量仍为 `unreviewed_skill_gate`。只检查目标自身：Global OAuth、Global PAT、CN OAuth 私有 receipt 各请求 Claude/Codex/Gemini 三份新鲜目录，但旧验收器只送 `app/providerId`；生产 `/v1/models` 实际先从 Router 签名 Share 和 App 选择 Provider，普通 `providerId` 不参与该选择。若 token 指向其他 Share 且目录有同名模型，旧请求不能证明 receipt 所声明的绑定。三个 rail 的正常回环 fixture 逐 App 增加预期 Share/Provider 查询断言后均先红；现在验收器发送 `expectedShareId` 与对应 App 的 `expectedProviderId`，借用已有签名 ingress 端到端测试覆盖的服务端匹配/冲突栅栏。每条 rail 还断言三 App 各请求一次，避免重复请求一类目录冒充三份。

本项只收紧私有目录证据，不改变生产 COSY wire、账号/站点/rail 选择或 receipt schema v2/revision 2；目录绑定也不是厂商 entitlement 或跨 API 原子快照。Qoder 专项 Node 48/48、全量 audit Node 496/496、smoke Node 13/13、签名目录路由 Rust 定点测试、rustfmt、Qoder CLI oracle 与默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查均通过；未运行受技能门禁限制的外仓 `--check-sources`。三个 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`，三条真实 receipt 分别缺失。当前 JS/文档增量未提交，不能用旧 committed-target observation 代表当前工作树；最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.54 CodeBuddy 双站三 Surface 目录的签名 Share / Provider 验收绑定

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` / tree `876656d0780afde26af9b1ab0a87e0ea58d8c752` 的提交态未变，未跟踪 `proxy.html`、`proxy.md` 继续排除。参考 Gateway 的公开 `/v1/models` 是展示目录聚合，不是目标 Server 的签名 Share 绑定证明。目标 Intl/CN 私有 receipt validator 虽分别检查三个 Provider 与 Share 的控制面绑定，三份目录请求却仅传 `app/providerId`；生产 `/v1/models` 实际从 Router 签名 Share 和 App 选 Provider，不由普通 `providerId` 查询决定。若 Router token 指向另一 Share 且有同名模型，旧脚本不能证明目录属于 receipt 声称的绑定。Intl/CN 正常 fixture 增加逐 App 预期 Share/Provider 请求断言后双站均先红；现在请求带 `expectedShareId/expectedProviderId`，复用服务端已有的签名 ingress 匹配/冲突栅栏。每站另要求 Claude/Codex/Gemini 各请求一次，不能重复一类目录冒充三份。

这只增强私有目录证据，不改变生产 CodeBuddy wire、站点/账号选择、目录 TTL 或 receipt schema v2/revision 2；也不证明厂商 entitlement 或跨 API 原子快照。CodeBuddy 专项 Node 38/38、全量 audit Node 498/498、smoke Node 13/13、签名目录路由 Rust 定点测试、rustfmt、CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查均通过。Intl/CN 的 `RUN_REAL=0` 各自仍为 `blocked_inputs/live_pending`，真实 receipt 及 CB-N4 厂商目录/接受性仍缺，`deepseek-v4.1-flash` 继续 disabled。当前工作树未提交，不能用既有 committed-target observation 冒充本轮实现证据；最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.55 Antigravity 双 rail 双目录的签名 Share / Provider 验收绑定

2026-10-08，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的提交态未变；既有 AG-N12/AG-D3 离线结论仍有效。本项核查目标自身私有 receipt：`antigravity_oauth`、`agy_oauth` 各要求 Claude/Gemini 两份新鲜目录，但旧请求只带 `app/providerId`。Server `/v1/models` 先按 Router 签名 Share 与 App 选 Provider，普通 `providerId` 查询不决定选择；如果 token 指向另一 Share 且目录有同名模型，旧验收无法证明所选 Share/Provider。双 rail 正常回环 fixture 对每 App 增加预期 Share/Provider 查询断言后均先红；现在请求带 `expectedShareId/expectedProviderId`，使用已有签名 ingress 匹配/冲突栅栏，且每条 rail 要求 Claude/Gemini 各请求一次。

本项只增强私有目录证据，不改变生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 决策或 receipt schema v2/revision 2；目录绑定也不证明 Google 厂商 entitlement 或跨 API 原子快照。Antigravity 专项 Node 46/46、全量 audit Node 498/498、smoke Node 13/13、签名目录路由 Rust 定点测试、rustfmt、Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法和差分空白检查均通过。`antigravity_oauth`、`agy_oauth` 的 `RUN_REAL=0` 分别为 `blocked_inputs/live_pending`；两条真实 receipt 与 AG-N6/compaction 专项接受性仍缺。当前 JS/文档增量未提交，不能用旧 committed-target observation 冒充本轮实现证据；最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.56 Claude 五 operation 模型目录的签名 Share / Provider 验收绑定

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` / tree `f43e8e15ea31e3d1dd1cf83250d9fdb627c06525` 的提交态未变，CL-N7/CL-N8 离线结论不变。目标五项私有 receipt validator 都请求一次 `/v1/models`，旧查询只有 `app=claude` 和普通 `providerId`；生产 Server 却先按 Router 签名 Share/App 选 Provider，不使用普通 `providerId` 选择。若 token 属于另一 Share 而静态目录同名，旧验收无法证明所选 operation 的目录作用域。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 五个独立正常 fixture 增加“请求包含所选 Share/Provider”断言后均先红；现在请求带 `expectedShareId/expectedProviderId`，使用已有签名 ingress 匹配/冲突栅栏，每项也要求恰好一次目录请求。

本项只增强私有目录路由证据，不改变 Claude OAuth 生产 wire、续线程、`pause_turn`、plan projection 或 receipt schema v1/revision 1；静态目录命中不证明真实订阅 entitlement，也不证明跨 API 原子快照。Claude 专项 Node 73/73、全量 audit Node 498/498、smoke Node 13/13、签名目录路由 Rust 定点测试、rustfmt、Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法和差分空白检查均通过。五项 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`；真实 receipt 与 thread/beta 厂商接受性分别缺失。当前 JS/文档增量未提交，不能用旧 committed-target observation 代表本轮实现；最终整体 review 仍未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.57 Codex Astra/WS 前置普通目录的签名 Share / Provider 验收绑定

2026-10-08，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交状态不变。目标 `gpt_6_astra` 与 `ws_prewarm` 私有验收已有签名绑定账号 manifest 查询及其 `expectedShareId/expectedProviderId` 栅栏，但两项在 manifest 前的普通 `/v1/models?app=codex` 仍只写普通 `providerId`；Server 实际按 Router 签名 Share 选 Provider，不由该查询参数决定。若 token 指向另一 Share 且目录有同名模型，旧前置目录不证明所选作用域。两项“普通目录也须声明所选 Share/Provider”的独立回环测试先红；现在两条普通目录请求补上预期 ID，并要求各恰好一次，复用服务端已有签名 ingress 匹配/冲突测试。

本项只补齐普通目录与账号 manifest 的一致验收作用域，不改生产 Codex HTTP/WS/Image wire、manifest 上游参数、receipt schema v1/revision 1 或三项 Images disabled 决策。官方 [OpenAI Docs 的 List models](https://developers.openai.com/api/reference/resources/models/methods/list) 仅描述平台模型列表，不是 ChatGPT 订阅 entitlement 或 Router token 的证明。Codex 专项 Node 51/51、全量 audit Node 501/501、smoke Node 13/13、签名目录路由 Rust 定点测试、rustfmt、Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法和差分空白检查均通过。五项 `RUN_REAL=0` 各自仍为 `blocked_inputs/live_pending`，真实 receipt 均缺；当前 JS/文档未提交，不能用旧 committed-target observation 冒充本轮实现。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.58 Cursor API Key 动态目录的接受前 TTL 复核

2026-10-08，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的提交态未变，22 项未提交 Codex 文件继续排除，CUR-N2 `reviewed_no_wire_delta` 不变。目标 OAuth/API Key 两 rail 私有验收已核对 Router 签名 Share/Provider；API Key 目录也要求 `cursor_public_api`、非 stale 和五分钟 TTL，但旧脚本只在取到目录时检查一次，之后解析 receipt 并末次读取 Provider/Share。一个目录获取时距过期还差一秒、末次 Share 响应延迟 1.5 秒的本地 fixture 先错误输出 `contract_verified/live_pending`。现在把 API Key 时间戳判定收敛为同一谓词，获取时和末次绑定复核后各检查一次，过期即拒绝；OAuth 的静态别名目录不套动态 TTL。Cursor 专项 Node 43/43。

这仅收紧私有验收时效，不修改生产 Cursor protobuf/session/目录发布 wire、两 rail 身份或 receipt schema v2/revision 2；末次快照仍不是跨 API 原子事务。Cursor 专项 Node 43/43、全量 audit Node 502/502、smoke Node 13/13、Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法和差分空白检查均通过；本轮只有 JS/文档改动，未重跑无变化的全量 Rust。OAuth/API Key 的 `RUN_REAL=0` 各自仍为 `blocked_inputs/live_pending`，真实 receipt 分别缺失。当前 JS/文档未提交，不能用旧 committed-target observation 冒充本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.59 Grok 三项私有目录的接受前时效复核

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态不变；后者未提交 Codex 文件继续排除。目标 inference、media、remote_compaction 私有验收原先只在取得目录时检查两小时新鲜窗口，之后解析 receipt 并末次复核 Account/Provider/Share。如果目录在此期间过期，旧流程仍会接受。三项各以距过期一秒的目录、延迟 1.5 秒的末次 Share 响应构造独立回环反例，先复现错误接受；现复用同一时间谓词，在末次绑定复核之后再检查目录时间，三项均失败关闭，Grok 专项 Node 49/49。

这仅收紧私有证据的接受时效，不改变生产 Grok 目录 TTL、HTTP/WS/media wire、默认 `0.2.111`、group header、receipt schema v1/revision 1 或 remote compaction disabled。末次控制面读取并非跨 API 原子事务，离线 fixture 不证明真实 xAI entitlement。Grok 专项 Node 49/49、全量 audit Node 506/506、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 和差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。inference、media、remote_compaction 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，三份真实 receipt 与 GR-N4 同账号版本/header 接受性仍缺。当前工作树未提交，不能把旧 committed-target observation 当作本轮实现证据；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.60 Kiro 八路双目录的接受前时效复核

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的提交态不变，KI-N5 context usage 仍仅作 shadow。目标八个 auth-kind×region 私有 receipt 各用 Claude/Codex 两份五分钟新鲜目录，但旧验收器只在取得目录时核对时间，之后解析 receipt 并末次复核 Account、双 Provider、Share。每个 scope 分别让 Claude 或 Codex 目录在取得时距过期一秒，延迟末次 Share 响应 1.5 秒；16 个本地回环反例先全部错误接受。现在初次与末次复核共用五分钟/未来一分钟时间谓词，末次绑定读取后分别复核两份目录，16 例均失败关闭，Kiro 专项 Node 114/114。

本项只收紧私有验收时效，不改变生产 Kiro/Amazon Q EventStream、目录 TTL、auth kind/region 路由、KI-N5 计量权威或 receipt schema v1/revision 1；Compact 与 shared cache 仍 disabled。末次控制面读取不构成跨 API 原子事务，也不证明真实厂商接受性。Kiro 专项 Node 114/114、全量 audit Node 523/523、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 和差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。八个 `RUN_REAL=0` 入口逐一为 `blocked_inputs/live_pending`，八份真实 receipt 和 KI-N5 专项证据仍缺。当前工作树未提交，不能用旧 committed-target observation 代表新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 业务源码仍受缺失的 `project-doc` 技能门禁。

### 18.61 Qoder 三 rail 三 Surface 目录的 receipt 发布前时效复核

2026-10-08，Qoder 单类别接续。推荐 TokenRouter 最新业务源码仍要求本会话不可用的 `project-doc` 技能；本项没有读取其业务对象，也没有运行访问外仓的 `--check-sources`，最新增量仍为 `unreviewed_skill_gate`。只检查目标自己的 Global OAuth、Global PAT、CN OAuth happy-path receipt 发布器：三 Surface 目录在取得时已要求两小时内新鲜，但随后还执行 quota、六个数据面请求及末次 Account/三 Provider/Share 复核，旧流程不再检查目录时间。每条 rail 让 Claude、Codex、Gemini 之一在取得时距过期 1.2 秒、末次 Share 响应延迟 2 秒；九个独立回环反例先全部错误写出 receipt。现在获取与发布前共用时间谓词，写文件前重查三份目录；任一过期便拒绝落盘，Qoder 专项 Node 58/58。

这只收紧私有 happy-path 证据时效，不改变生产 COSY wire、目录 TTL、rail/site/Account 选择或 receipt schema v2/revision 2；末次读取不是跨 API 原子事务，也不证明上游厂商接受性。Global OAuth、Global PAT、CN OAuth 的真实 receipt 及登录/轮换、权威空目录和受控 401 等独立门槛仍缺，happy-path 即使真实成功也只能 `partial_live_verified/live_pending`。Qoder 专项 Node 58/58、全量 audit Node 533/533、smoke Node 13/13；CLI oracle、自包含默认 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。没有运行受技能门禁约束的外仓 `--check-sources`；本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。三条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，无真实请求或 receipt。当前工作树未提交，不能用旧 committed-target observation 代表新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.62 CodeBuddy Intl/CN 私有 receipt 的最终接受时效复核

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交状态未变，未跟踪 `proxy.html`、`proxy.md` 继续排除；本项没有新的厂商 wire 推断。目标三份目录已在末次绑定复核后重查八分钟 TTL，但私有 receipt 的 `recordedAt` 只在内容校验时检查 24 小时窗口，之后还重读 Account/Provider/Share。Intl、CN 各以距过期 1.2 秒的 receipt 与延迟 2 秒的末次 Share 响应构造本地回环反例，先均错误输出 `contract_verified/live_pending`。现在内容校验和末次绑定复核后共用同一 receipt 时间谓词，过期即脱敏拒绝；CodeBuddy 专项 Node 41/41。

这只收紧私有证据时效，不更改生产 CodeBuddy wire、目录 TTL、Intl/CN 身份、receipt schema v2/revision 2 或 CB-N4 `deepseek-v4.1-flash` disabled 决策。末次快照不是跨 API 原子事务，也不证明厂商接受性。CodeBuddy 专项 Node 41/41、全量 audit Node 536/536、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，两站真实 receipt 和 CB-N4 CN 目录/接受性证据仍缺。当前工作树未提交，不能用旧 committed-target observation 冒充本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.63 Antigravity/AGY 双目录与 receipt 的最终接受时效复核

2026-10-08，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交状态不变；本项不从外仓推断新 Google wire。目标 `antigravity_oauth`、`agy_oauth` 的 v2 私有 receipt 已在取得 Claude/Gemini 目录时检查两小时窗口、内容校验时检查 receipt 24 小时窗口，但之后还进行末次 Account/Provider/Share 复核，不再查证据时效。两条 rail 分别使 Claude/Gemini 某一目录或 receipt 在开始时距过期 1.2 秒，延迟末次 Share 响应 2 秒；六个独立回环反例先全部错误接受。现在复用同一目录与 receipt 时间谓词，在最终绑定读取后以共同接受时刻核对三份证据，六例均失败关闭，Antigravity 专项 Node 53/53。

本项只提高私有验收时效，不改变生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction、目录发布或 receipt schema v2/revision 2；末次读取不是跨 API 原子事务，离线 fixture 不能证明 Google 接受性。Antigravity 专项 Node 53/53、全量 audit Node 543/543、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6 和 compaction 专项接受性及新实现提交态证据仍缺。当前工作树未提交，不能把旧 committed-target observation 当作本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.64 Claude 五 operation receipt 与套餐证据的最终接受时效复核

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交状态未变，CL-N7/CL-N8 的离线差异结论不变。本仓库五项私有 receipt 已在内容校验时要求 `recordedAt` 处于 24 小时窗口，Max 5x/20x 与 Fable 5.1 还在强制 quota refresh 后要求 canonical plan `observedAt` 处于 15 分钟窗口，但最后会重读 Account/Provider/Share，旧流程在这段时间内不再检查证据时效。五份 receipt、三份 plan 各设为距过期 1.2 秒，并延迟末次 Share 响应 2 秒；八个独立本地反例先全部错误接受。现在初次与末次接受共用时间谓词，在最终绑定读取后以同一时刻核对 receipt 以及适用 operation 的 plan；八例均失败关闭，Claude 专项 Node 82/82。Opus 5.5 的 access-only 路径不被错误套用套餐门槛。

这只收紧私有证据时效，不改变生产 Claude OAuth wire、续线程/`pause_turn`、计划投影或 receipt schema v1/revision 1；末次读取不是跨 API 原子事务，也不证明厂商接受性。Claude 专项 Node 82/82、全量 audit Node 552/552、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。五项 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，五份真实 receipt 和 CL-N7/CL-N8 接受性仍独立缺失。当前工作树未提交，不能用旧 committed-target observation 代表本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.65 Codex 五 operation 私有 receipt 的最终接受时效复核

2026-10-08，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交状态不变，本项不推导新的 OpenAI wire。目标 Astra、三个 Image 2.5 变体、WS prewarm 的私有 receipt 原先只在内容校验时检查 `recordedAt` 的 24 小时窗口，随后还会末次重读 Account/Provider/Share。五项各以距过期 1.2 秒的 receipt、延迟 2 秒的末次 Share 响应构造回环反例，旧流程五例全部错误接受。现在复用同一时间谓词，在末次绑定读取后再次核对 receipt，五例均脱敏失败关闭，Codex 专项 Node 57/57。

这只收紧私有证据接受时效，不改变生产 Codex HTTP/WS/Image wire、receipt schema v1/revision 1 或 Images/WS prewarm 的 disabled/未启用决策；末次读取不是跨 API 原子事务，离线 fixture 不证明真实账号 entitlement 或性能收益。Codex 专项 Node 57/57、全量 audit Node 558/558、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。五项 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，五份独立真实 receipt 仍缺。当前工作树未提交，不能以旧 committed-target observation 冒充新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.66 Cursor OAuth/API Key receipt 的最终接受时效复核

2026-10-08，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交状态不变，22 项未提交 Codex 文件继续排除，CUR-N2 `reviewed_no_wire_delta` 不变。目标 API Key 动态目录已在末次绑定复核后重查五分钟 TTL，但 OAuth/API Key 私有 receipt 的 `recordedAt` 仍只在内容校验时检查 24 小时窗口。两 rail 各以距过期 1.2 秒的 receipt、延迟 2 秒的末次 Share 响应构造回环反例，旧流程均错误接受。现在两 rail 的 receipt 在末次绑定读取后再检查，API Key 目录与 receipt 使用同一接受时刻；两例脱敏失败关闭，Cursor 专项 Node 46/46。

这只收紧私有证据时效，不改变生产 Cursor protobuf/session、两 rail 身份、目录发布或 receipt schema v2/revision 2；OAuth 静态目录不套用 API Key 动态 TTL。末次读取不是跨 API 原子事务，fixture 不证明真实厂商 entitlement。Cursor 专项 Node 46/46、全量 audit Node 561/561、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。两 rail `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，两份真实 receipt 仍缺。当前工作树未提交，不能用旧 committed-target observation 代表新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.67 Grok 三 operation 私有 receipt 的最终接受时效复核

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态未变；后者未提交 Codex 文件仍排除。目标 inference、media、remote_compaction 的目录证据已在末次绑定读取后重查两小时窗口，但 receipt 的 `recordedAt` 仍只在内容校验时检查 24 小时窗口。三项分别设为距过期 1.2 秒的 receipt，末次 Share 响应延迟 2 秒；旧流程三例全部错误接受。现在初次与末次复核共用 receipt 时间谓词，并以同一接受时刻核对目录和 receipt；三例均脱敏失败关闭，Grok 专项 Node 53/53。

这仅收紧私有证据时效，不改变生产 Grok CLI 默认 `0.2.111`、group header、HTTP/WS/media wire、目录 TTL、receipt schema v1/revision 1 或 remote compaction disabled。末次快照不是跨 API 原子事务，离线 fixture 不证明真实 xAI entitlement。Grok 专项 Node 53/53、全量 audit Node 565/565、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。三项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，GR-N4 同账号版本/header 接受性和三份真实 receipt 仍缺。当前工作树未提交，不能以旧 committed-target observation 冒充新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.68 Kiro 八路私有 receipt 的最终接受时效复核

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交状态未变，KI-N5 context usage 仍仅 shadow。目标八个 auth-kind×region scope 的 Claude/Codex 目录已在末次绑定读取后重查五分钟窗口，但 receipt `recordedAt` 只在内容校验时检查 24 小时窗口。Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 各把 receipt 设为距过期 1.2 秒，并延迟末次 Share 响应 2 秒；旧流程八例均错误接受。现在初次与末次复核共用 receipt 时间谓词，在最终绑定读取后以同一接受时刻核对两份目录与 receipt；八例均脱敏失败关闭，Kiro 专项 Node 123/123。

本项只收紧私有证据时效，不改变生产 Kiro/Amazon Q EventStream、auth kind/region 路由、KI-N5 计量权威或 receipt schema v1/revision 1；Compact/shared cache 继续 disabled。末次读取不是跨 API 原子事务，离线 fixture 不证明厂商接受性。Kiro 专项 Node 123/123、全量 audit Node 574/574、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。八个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，八份真实 receipt 与 KI-N5 专项证据仍缺。当前工作树未提交，不能以旧 committed-target observation 代表新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 源码仍受 `project-doc` 技能门禁。

### 18.69 Qoder 三 rail 私有 receipt 临时写入后的目录时效复核

2026-10-08，Qoder 单类别接续。推荐 TokenRouter 最新业务源码仍要求本会话不可用的 `project-doc` 技能；没有读取该仓业务对象或运行访问外仓的 `--check-sources`，最新增量保持 `unreviewed_skill_gate`。目标 Global OAuth、Global PAT、CN OAuth 的 receipt 原已在末次绑定后重查三份目录时效，但随后仍需写临时文件、设权限和排他发布。三条 rail 各把 Claude 目录设为距两小时上限 1.2 秒，并让临时文件写入耗时 2 秒；旧流程三例均错误写出 `contract_verified/live_pending` receipt。现在保留写入前检查，并在临时文件写完、设好 `0600` 权限之后、硬链接排他发布之前用同一时间谓词再检查；三例都拒绝发布并清理临时文件，正常三 rail 和已有排他发布 fixture 保持通过，Qoder 专项 Node 62/62。

本项仅收紧私有 happy-path 证据发布门槛，不改变生产 COSY wire、目录 TTL、Account/Provider/Share 选择或 receipt schema v2/revision 2；检查与硬链接之间不是跨 API/文件系统原子快照，也不证明真实厂商接受性。Qoder 专项 Node 62/62、全量 audit Node 578/578、smoke Node 13/13；Qoder CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。未运行受技能门禁限制的外仓 `--check-sources`；本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，Global OAuth、Global PAT、CN OAuth 的三份真实 receipt 与登录/轮换、权威空目录、受控两段 401 等独立门槛仍缺。当前工作树未提交，不能以旧 committed-target observation 代表新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.70 CodeBuddy Intl/CN 私有 receipt 读取期间原位改写栅栏

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交状态未变，两个未跟踪 `proxy.html`、`proxy.md` 继续排除；本项不推导新的厂商 wire。目标私有 receipt 已在控制面请求前通过只读描述符限量读取，但只检查初始文件身份、读满与 EOF。Intl/CN 各在首次读取有效字节后、EOF 检查前把同 inode 文件等长原位覆盖并改变 mtime；旧流程均继续使用旧字节，错误输出 `contract_verified/live_pending`。现在同一描述符在读取前后比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；两例在任何控制面请求前脱敏失败，正常双站与已有文件替换/硬链接、目录时效、末次绑定 fixture 均通过，CodeBuddy 专项 Node 44/44。

本项只收紧私有证据文件的快照稳定性，不改变生产 CodeBuddy wire、Intl/CN 站点身份、Account/Provider/Share 选择、目录 TTL、receipt schema v2/revision 2 或 CB-N4 `deepseek-v4.1-flash` disabled 决策；元数据复原与并发精确竞态不由此证明，也不是厂商接受性。CodeBuddy 专项 Node 44/44、全量 audit Node 581/581、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，两站真实 receipt 与 CB-N4 CN 目录/接受性证据仍缺。当前工作树未提交，不能用旧 committed-target observation 代表新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.71 Antigravity/AGY 双 rail 私有 receipt 读取期间原位改写栅栏

2026-10-08，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交状态未变；本项没有新的 Google wire 推断。目标双 rail 私有 receipt 已在控制面请求前限量快照，但只核对初始文件身份、读满与 EOF。`antigravity_oauth`、`agy_oauth` 各在首次读取有效字节后、EOF 检查前把同 inode 文件等长原位覆盖并改变 mtime；旧流程两例均继续使用旧字节，错误输出 `contract_verified/live_pending`。现在同一只读描述符在读取前后比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；两例在任何控制面请求前脱敏失败，正常双 rail 和已有文件替换/硬链接、目录时效、末次绑定 fixture 保持通过，Antigravity 专项 Node 56/56。

本项只收紧私有证据文件快照稳定性，不改变生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction、目录发布或 receipt schema v2/revision 2；元数据复原与并发精确竞态不由此证明，也不构成真实 Google 接受性。Antigravity 专项 Node 56/56、全量 audit Node 584/584、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，两份真实 receipt、AG-N6 和 compaction 专项证据仍缺。当前工作树未提交，不能以旧 committed-target observation 代表新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.72 Claude 五 operation 私有 receipt 读取期间原位改写栅栏

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交状态未变，CL-N7/CL-N8 的离线结论不变。目标五项 receipt 已在控制面请求前限量快照，但只检查初始文件身份、读满与 EOF。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 各在首次读取有效字节后、EOF 检查前把同 inode 文件等长覆盖并改变 mtime；旧流程五例均继续使用旧字节，错误输出 `contract_verified/live_pending`。现在同一只读描述符在读取前后比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；五例均在任何控制面请求前脱敏失败，正常五项与已有文件替换/硬链接、plan projection、末次绑定 fixture 保持通过，Claude 专项 Node 88/88。

本项只收紧私有证据文件快照稳定性，不改变生产 Claude OAuth wire、CL-N7/CL-N8、plan projection 或 receipt schema v1/revision 1；元数据复原与并发精确竞态不由此证明，也不构成 Anthropic 接受性。Claude 专项 Node 88/88、全量 audit Node 590/590、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。五项 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，五份真实 receipt 及续线程/`pause_turn` 的厂商接受性仍缺。当前工作树未提交，不能以旧 committed-target observation 代表新逻辑；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.73 Codex 五 operation 私有 receipt 读取期间原位改写栅栏

2026-10-08，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交状态不变，本项不推导新 OpenAI wire。目标五项 receipt 已在控制面请求前限量快照，但旧逻辑只检查初始文件身份、读满和 EOF。`gpt_6_astra`、三个 Image 2.5 变体、`ws_prewarm` 各在首次读取有效字节后、EOF 前把同 inode 文件等长原位覆盖并改变 mtime；旧流程五例均继续用旧字节错误输出 `contract_verified/live_pending`。现在同一只读描述符在读取前后比较 dev/inode、链接数、大小、权限与纳秒级 mtime/ctime；五例在任何控制面请求前脱敏失败，正常五项与既有文件替换/硬链接、时效、末次绑定 fixture 保持通过。

本项仅收紧私有证据文件快照稳定性，不改变生产 Codex HTTP/WS/Image wire、receipt schema v1/revision 1 或 Images/WS prewarm disabled 决策；元数据复原与精确并发竞态下的原子快照不由此证明，也不是实际账号 entitlement。Codex 专项 Node 63/63、全量 audit Node 596/596、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。五项 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`，五份独立真实 receipt 仍缺。当前工作树未提交，不能以旧 committed-target observation 冒充本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.74 Cursor OAuth/API Key 私有 receipt 读取期间原位改写栅栏

2026-10-08，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 HEAD 未变，22 项未提交 Codex 文件继续排除，CUR-N2 `reviewed_no_wire_delta` 结论不变。本项只检查目标自身私有 validator：OAuth/API Key receipt 原已在控制面请求前限量快照，但仅核对初始身份、读满和 EOF。两 rail 各在首次读取有效字节后、EOF 前把同 inode 文件等长原位覆盖并改变 mtime；旧流程均继续使用旧字节错误输出 `contract_verified/live_pending`。现在同一只读描述符在读取前后比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；两例在任何控制面请求前脱敏失败，正常双 rail 与已有路径替换/硬链接、目录时效、末次绑定 fixture 保持通过。

本项只收紧私有证据快照稳定性，不改变生产 Cursor protobuf/session/目录发布 wire、OAuth/API Key 身份或 receipt schema v2/revision 2；元数据复原和精确并发竞态下的原子快照不由此证明，也不是厂商 entitlement。Cursor 专项 Node 49/49、全量 audit Node 599/599、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。OAuth、API Key 的 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`，两份真实 receipt 分别缺失。当前工作树未提交，不能用旧 committed-target observation 代表本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.75 Grok 三 operation 私有 receipt 读取期间原位改写栅栏

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交 HEAD 不变，本项不从外仓推断新的 xAI wire。目标 inference、media、remote_compaction 私有 receipt 原已在控制面请求前限量快照，但仅检查初始身份、读满与 EOF。三项各在首次读取有效字节后、EOF 前把同 inode 文件等长原位覆盖并改变 mtime；旧流程均继续使用旧字节错误输出 `contract_verified/live_pending`。现在同一只读描述符在读取前后比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；三例均在任何控制面请求前脱敏失败，正常三项与既有路径替换/硬链接、目录时效、末次绑定 fixture 保持通过。

本项只收紧私有证据快照稳定性，不改变生产 Grok CLI 默认 `0.2.111`、group header、HTTP/WS/media wire、receipt schema v1/revision 1 或 remote compaction disabled 决策；元数据复原与精确并发竞态下的原子快照不由此证明，也不构成真实 xAI entitlement。Grok 专项 Node 57/57、全量 audit Node 603/603、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。三项 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`；三份真实 receipt 与 GR-N4 同账号版本/header 接受性证据仍缺。当前工作树未提交，不能用旧 committed-target observation 代表本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.76 Kiro 八路私有 receipt 读取期间原位改写栅栏

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交 HEAD 不变，本项不从外仓推断新的 EventStream wire。目标 Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 八个私有 receipt 原已在控制面请求前限量快照，但只核对初始身份、读满及 EOF。每个 scope 在首次读取有效字节后、EOF 前把同 inode 文件等长原位覆盖并改变 mtime；旧流程八例均继续使用旧字节错误输出 `contract_verified/live_pending`。现在同一只读描述符在读取前后比较 dev/inode、链接数、大小、权限和纳秒级 mtime/ctime；八例在控制面请求前脱敏失败，正常八路与既有路径替换/硬链接、双目录时效、末次绑定 fixture 保持通过。

本项只收紧私有证据快照稳定性，不改变生产 Kiro/Amazon Q EventStream、auth kind/region 路由、KI-N5 usage authority、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策；元数据复原及精确并发竞态下的原子快照不由此证明，也不构成厂商接受性。Kiro 专项 Node 132/132、全量 audit Node 612/612、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。八个 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`；八份真实 receipt 和 KI-N5 专项证据仍缺。当前工作树未提交，不能用旧 committed-target observation 代表本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 业务源码仍受 `project-doc` 技能门禁。

### 18.77 Qoder 三 rail 私有 receipt 父目录交换栅栏

2026-10-08，Qoder 单类别接续。推荐 TokenRouter 仓库的 `AGENTS.md` 要求当前会话先调用不可用的 `project-doc` 技能；本项没有读取其业务源码、执行仓库命令或运行外仓 `--check-sources`，最新增量继续 `unreviewed_skill_gate`。只检查本仓库自己的 Global OAuth、Global PAT、CN OAuth receipt 发布器：旧逻辑在真实路径检查后仍用字面父目录写临时文件、做排他硬链接。三条 rail 各让仓库外符号链接父目录在首次临时文件写入前改指仓库内；旧流程均把 receipt 写进仓库并错误报告 `contract_verified/live_pending`。现在打开解析后的仓库外目录，通过 Linux `/proc/self/fd` 将临时文件与最终硬链接锚定于同一目录描述符；写入前、链接前及链接后复核配置路径与目录 inode、仓库外真实位置。三例写入期交换、三例链接后交换均失败且清除本次文件，正常三 rail、排他发布与目录时效 fixture 保持通过；缺少 Linux 描述符锚定时在控制面请求前失败关闭。

本项只收紧私有证据输出位置，不改变生产 COSY wire、Account/Provider/Share、site/rail 选择或 receipt schema v2/revision 2。目录 inode 本身被并发移动、恶意元数据复原或末次检查之后再次漂移仍不由此获得原子保证；fixture 不证明真实厂商接受性。Qoder 专项 Node 70/70、全量 audit Node 620/620、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。没有运行受 `project-doc` 技能门禁约束的外仓 `--check-sources`；本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。三个 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`，Global OAuth、Global PAT、CN OAuth 三份真实 receipt 与登录/轮换、权威空目录、受控 401 等独立门槛仍缺。当前工作树未提交，不能用旧 committed-target observation 代表本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.78 CodeBuddy Intl/CN 私有 receipt 最终文件身份复核

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD 不变，两个未跟踪 `proxy.html`、`proxy.md` 继续排除；本项不推导新的厂商 wire。目标双站 receipt 已在控制面请求前读取稳定文件快照，但旧流程随后只校验内存内容和 Account/Provider/Share/目录时效，最终不确认配置路径仍指向所验收的文件。Intl/CN 各在首个 Account 读取时把原有效文件替换为另一个 inode 的无效文件，或将原 inode 等长覆盖并改变 mtime；旧流程四例均继续接受旧内存内容，错误输出 `contract_verified/live_pending`。现在保存快照后的文件指纹，并在最终绑定及证据时效复核后重新解析配置路径，比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；四例均脱敏失败，正常双站和既有文件/绑定/目录 fixture 保持通过。

本项只收紧私有证据文件在验收结束时的稳定性，不改变生产 CodeBuddy wire、Intl/CN 站点身份、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled 决策；末次文件检查不是跨 API/文件系统原子事务，恶意元数据复原与检查之后的变化不由此证明，离线 fixture 亦非厂商接受性。CodeBuddy 专项 Node 49/49、全量 audit Node 625/625、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。Intl/CN 的 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`，两份真实 receipt 与 CB-N4 CN 目录/接受性证据仍缺。当前工作树未提交，不能以旧 committed-target observation 冒充本轮实现；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.79 Antigravity/AGY 双 rail 私有 receipt 最终文件身份复核

2026-10-08，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD 不变；AG-N12/AG-D3 已有本地离线设计和差分结论，本项不新增 Google wire。目标 `antigravity_oauth`、`agy_oauth` 私有 receipt 已在控制面请求前读取稳定快照，但旧流程随后只核对内存内容、Account/Provider/Share 及证据时效，最终不确认配置路径仍指向所验收的文件。双 rail 各在首个 Account 读取时把原有效文件替换为另一个 inode 的无效文件，或将原 inode 等长覆盖并改变 mtime；旧流程四例均继续接受旧内存内容，错误输出 `contract_verified/live_pending`。现在保存快照后的文件指纹，并在最终绑定和证据时效复核后重新解析配置路径，比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；四例均脱敏失败，正常双 rail 与既有文件/绑定/目录 fixture 保持通过。

本项只收紧私有证据文件在验收结束时的稳定性，不改变生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 或 receipt schema v2/revision 2；末次文件检查不是跨 API/文件系统原子事务，恶意元数据复原及检查后的变化不由此证明，fixture 亦非真实 Google 接受性。Antigravity 专项 Node 61/61、全量 audit Node 630/630、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。两条 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6/compaction 专项证据及当前工作树的提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.80 Claude 五 operation 私有 receipt 最终文件身份复核

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交状态未变；CL-N7/CL-N8 的离线结论不变，本项不推导新的 Anthropic wire。五项私有 receipt 已在控制面请求前读取稳定快照，但旧流程随后只校验内存内容、Account/Provider/Share 和证据时效，不确认配置路径最终仍指向所验收的文件。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 各在首次 Account 读取时把原有效文件替换为另一个 inode 的无效文件，或将原 inode 等长覆盖并改变 mtime；旧流程十例全部错误输出 `contract_verified/live_pending`。现在保存快照后的文件指纹，并在最终绑定与证据时效复核后重新解析配置路径，比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；十例均脱敏失败，正常五项与既有文件、绑定、套餐和时效 fixture 保持通过。

本项只收紧私有证据文件在验收结束时的稳定性，不改变生产 Claude OAuth wire、CL-N7/CL-N8、plan projection 或 receipt schema v1/revision 1。末次文件检查不是跨 API/文件系统原子事务，恶意元数据复原及检查后的变化不由此证明，离线 fixture 亦非 Anthropic 接受性。Claude 专项 Node 99/99、全量 audit Node 641/641、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。五项 `RUN_REAL=0` 入口仍分别为 `blocked_inputs/live_pending`；五份真实 receipt、CL-N7/CL-N8 厂商接受性及当前工作树的提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.81 Codex 五 operation 私有 receipt 最终文件身份复核

2026-10-08，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交状态未变，本项不推导新的 OpenAI wire。Astra、三个 Image 2.5 变体与 WS prewarm 的私有 receipt 已在控制面请求前读取稳定快照，但旧流程随后只校验内存内容、Account/Provider/Share 和时效，不确认配置路径最终仍指向所验收的文件。五项各在首次 Account 读取时把原有效文件替换为另一个 inode 的无效文件，或将原 inode 等长覆盖并改变 mtime；旧流程十例全部错误输出 `contract_verified/live_pending`。现在保存快照后的文件指纹，最终绑定与时效复核后重新解析配置路径，比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；十例均脱敏失败，正常五项与既有文件、模型 manifest、绑定和时效 fixture 保持通过。

本项只收紧私有证据文件在验收结束时的稳定性，不改变生产 Codex HTTP/WS/Image wire、receipt schema v1/revision 1、Images disabled 或 WS prewarm 未启用决策。末次文件检查不是跨 API/文件系统原子事务，恶意元数据复原及检查后的变化不由此证明，离线 fixture 亦非真实账号 entitlement。Codex 专项 Node 74/74、全量 audit Node 652/652、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。五项 `RUN_REAL=0` 入口仍分别为 `blocked_inputs/live_pending`；五份真实 receipt 及当前工作树的提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.82 Cursor OAuth/API Key 私有 receipt 最终文件身份复核

2026-10-08，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交状态未变，22 项未提交 Codex 工作树内容继续排除，CUR-N2 `reviewed_no_wire_delta` 结论不变。双 rail 的私有 receipt 已在控制面请求前读取稳定快照，但旧流程随后只校验内存内容、Account/Provider/Share 与证据时效，不确认配置路径最终仍指向所验收的文件。OAuth/API Key 各在首次控制面读取时把原有效文件替换为另一 inode 的无效文件，或将原 inode 等长覆盖并改变 mtime；旧流程四例全部错误输出 `contract_verified/live_pending`。现在保存快照后的文件指纹，在最终绑定、API Key 目录与 receipt 时效复核后重新解析配置路径，比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；四例均脱敏失败，正常双 rail 与既有文件、目录、绑定和时效 fixture 保持通过。

本项只收紧私有证据文件在验收结束时的稳定性，不改变生产 Cursor protobuf/session、目录发布、OAuth/API Key 身份或 receipt schema v2/revision 2。末次文件检查不是跨 API/文件系统原子事务，恶意元数据复原及检查后的变化不由此证明；OAuth 静态别名仍不是账号 entitlement，离线 fixture 也不是厂商接受性。Cursor 专项 Node 54/54、全量 audit Node 657/657、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。两 rail `RUN_REAL=0` 仍分别为 `blocked_inputs/live_pending`；两份真实 receipt 及当前工作树的提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.83 Grok 三 operation 私有 receipt 最终文件身份复核

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态未变，后者六项未提交 Codex 工作树内容继续排除；本项不推导新的 xAI wire。inference、media、remote_compaction 的私有 receipt 已在控制面请求前读取稳定快照，但旧流程随后只校验内存内容、Account/Provider/Share、目录与证据时效，不确认配置路径最终仍指向所验收的文件。三项各在首次 Account 读取时把原有效文件替换为另一 inode 的无效文件，或将原 inode 等长覆盖并改变 mtime；旧流程六例全部错误输出 `contract_verified/live_pending`。现在保存快照后的文件指纹，最终绑定、目录与 receipt 时效复核后重新解析配置路径，比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；六例均脱敏失败，正常三项与既有文件、目录、绑定和时效 fixture 保持通过。

本项只收紧私有证据文件在验收结束时的稳定性，不改变生产 Grok CLI 默认 `0.2.111`、group header、HTTP/WS/media wire、receipt schema v1/revision 1 或 remote compaction disabled 决策。末次文件检查不是跨 API/文件系统原子事务，恶意元数据复原及检查后的变化不由此证明，离线 fixture 亦非真实 xAI entitlement。Grok 专项 Node 64/64、全量 audit Node 664/664、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。三项 `RUN_REAL=0` 仍分别为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 接受性及当前工作树的提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.84 Kiro 八路私有 receipt 最终文件身份复核

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交状态未变；本项不推导新的 Kiro/Amazon Q wire。Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 八路私有 receipt 已在控制面请求前读取稳定快照，但旧流程随后只校验内存内容、Account/双 Provider/Share、双目录及证据时效，不确认配置路径最终仍指向所验收的文件。八个 scope 各在首次 Account 读取时把原有效文件替换为另一 inode 的无效文件，或将原 inode 等长覆盖并改变 mtime；旧流程十六例全部错误输出 `contract_verified/live_pending`。现在保存快照后的文件指纹，最终绑定、双目录与 receipt 时效复核后重新解析配置路径，比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；十六例均脱敏失败，正常八路与既有文件、目录、绑定和时效 fixture 保持通过。

本项只收紧私有证据文件在验收结束时的稳定性，不改变生产 Kiro/Amazon Q EventStream、auth kind/region 路由、KI-N5 usage authority、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策。末次文件检查不是跨 API/文件系统原子事务，恶意元数据复原及检查后的变化不由此证明，离线 fixture 亦非厂商接受性。Kiro 专项 Node 149/149、全量 audit Node 681/681、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。八个 `RUN_REAL=0` 仍分别为 `blocked_inputs/live_pending`；八份真实 receipt、KI-N5 专项证据及当前工作树的提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 业务源码仍受 `project-doc` 技能门禁。

### 18.85 Qoder 三 rail 私有 receipt 发布内容复核

2026-10-08，Qoder 单类别接续。推荐 TokenRouter 当前 `AGENTS.md` 仍要求本会话不可用的 `project-doc` 技能；没有读取其业务源码、运行仓库业务命令或读取外仓源码的 `--check-sources`，最新增量继续 `unreviewed_skill_gate`。只检查本仓库 Global OAuth、Global PAT、CN OAuth 三条 rail 的私有 receipt 发布器：原流程在描述符锚定的仓库外父目录写临时文件、设权限并排他硬链接，但未核对发布字节仍是预期 receipt。三条 rail 各在临时写入后等长原位改写、替换临时路径，或在硬链接后原位改写，旧流程九例都错误输出 `contract_verified/live_pending`。新增的三条链接后路径替换反例还证明，仅加内容校验但沿用旧清理方式会误删后来占据目标路径的文件。

现在发布器将预期完整 receipt 字节保留在内存，在临时文件与最终硬链接上分别通过 `O_NOFOLLOW` 描述符核对普通文件、链接数、大小、`0600` 权限、dev/inode、纳秒级 mtime/ctime 和完整字节；发布后发现漂移时，只撤销仍属于本次文件 inode 的最终链接。十二例都拒绝成功报告，路径替换后的其他文件不被删除；正常三 rail 与排他发布、父目录交换、目录时效 fixture 保持通过。本项只提高私有证据发布门槛，不改变生产 COSY wire、site/rail/Account 选择、目录 TTL 或 receipt schema v2/revision 2。检查与硬链接不是原子事务，元数据复原、目录 inode 并发移动及末次检查后的变化仍不由此证明，fixture 也不是厂商接受性。Qoder 专项 Node 83/83、全量 audit Node 694/694、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法、rustfmt 与差分空白检查均通过；外仓 `--check-sources` 因技能门禁未运行。本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。三条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt 与登录/轮换、权威空目录、受控两段 401 等门槛及当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.86 CodeBuddy Intl/CN 私有 receipt 最终内容复核

2026-10-08，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD 不变，两个未跟踪文件 `proxy.html`、`proxy.md` 继续排除；只读核对其 WorkBuddy adapter/SSE 后，本项没有新的厂商 wire 推断。目标双站 receipt 已在控制面请求前保存原始字节与文件指纹，并在末次验收复核路径和元数据，但旧流程不重读内容。Intl/CN 各在首个 Account 请求时把 receipt 等长改写，并用测试预加载钩子让末次路径及描述符元数据读取返回原指纹；旧流程两例都错误输出 `contract_verified/live_pending`。元数据伪装只是验证“仅凭元数据不能证明内容”的对抗 fixture，不代表普通用户能够恢复真实 `ctime`。

现在末次检查以 `O_NOFOLLOW` 只读描述符重新读取完整、至多 1 MiB 的 receipt 字节及 EOF，与最初验收的原始字节比较，并在读取前后和路径处复核身份与元数据。两例均脱敏拒绝，正常 Intl/CN、旧有文件替换/原位改写、绑定、目录及时效 fixture 保持通过；CodeBuddy 专项 Node 52/52、全量 audit Node 697/697、smoke Node 13/13。CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 和差分空白检查通过；本轮只改 JS/文档，未重跑无变化的全量 Rust 套件。本项只提高私有验收证据门槛，不改变生产 CodeBuddy wire、site/Account/Share 选择、receipt schema v2/revision 2 或 CB-N4 disabled 决策；末次文件检查之后的并发变化及跨 API 原子性仍不保证，fixture 不是厂商接受性。Intl/CN 的 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，两份真实 receipt 与 CB-N4 CN 厂商目录/接受性证据仍缺；当前工作树未提交，不能用旧 committed-target observation 冒充本轮实现。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.87 Antigravity/AGY 双 rail 私有 receipt 内容复核与目录合同纠偏

2026-10-08，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD 未变；本项没有新的 Google wire 推断。目标 `antigravity_oauth`、`agy_oauth` 私有 receipt 已先取稳定快照、末次复核路径和文件指纹，但旧流程不重读内容。两条 rail 各在首个 Account 请求时把文件等长改写，并用测试预加载钩子让末次路径及描述符元数据读数保持原指纹；旧流程两例均错误输出 `contract_verified/live_pending`。钩子的元数据伪装仅验证不能把元数据当成内容证明，不表示普通用户能恢复真实 `ctime`。

现在末次通过 `O_NOFOLLOW` 只读描述符重新读取最多 1 MiB 的完整原始字节及 EOF，核对读前读后指纹与配置路径，内容改变即脱敏拒绝。Antigravity 专项 Node 64/64、全量 audit Node 700/700、smoke Node 13/13；reference-delta `--check-sources`、Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。另核对生产 `src/clients/oauth/antigravity_models.rs`：AG-N12 已把目录缓存键扩为 Provider type、Account id/incarnation、auth/token 两代际、project id、source URL，并用 request revision 栅栏拒绝晚返回旧结果；生成的 coverage 叙述及真实验收 runbook 仍写成只隔离 Provider/Account/auth 代际。已修改生成脚本并重新生成 coverage 文档，runbook 同步要求精确 scope、删除重建及 A/B 倒序验收；生成资产检查和文档索引复核通过。

本轮只改私有 JS 验收器、测试与文档/生成器，不改生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 或 receipt schema v2/revision 2；末次文件检查后竞态与跨 API 原子性仍不保证，fixture 不能证明 Google 厂商接受性。两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，真实 receipt 与 AG-N6/compaction 专项证据仍缺；当前工作树未提交，不把旧 committed-target observation 当作本轮实现证据。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.88 Claude 五 operation 私有 receipt 最终内容复核

2026-10-08，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD 未变；CL-N7 续线程 alias、CL-N8 `pause_turn` 已有离线实现，但本项没有新的 Anthropic wire 推断。目标 `oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 五项 receipt 已在控制面请求前保存稳定字节和指纹，并在最终接受前检查路径与元数据，却没有重读内容。五项各在首个 Account 请求时等长改写 receipt，再以测试预加载钩子让末次路径及描述符元数据读取返回原指纹；旧流程五例全部错误输出 `contract_verified/live_pending`。钩子只是对抗 fixture，不代表普通用户能够恢复真实 `ctime`。

现在最终检查通过 `O_NOFOLLOW` 只读描述符重读完整、至多 1 MiB 的原始 receipt 字节和 EOF，并复核读前读后指纹及配置路径；五例均脱敏失败，正常五项、旧有替换/原位改写、计划投影、绑定及证据时效 fixture 保持通过。Claude 专项 Node 105/105、全量 audit Node 706/706、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 和差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；生产 Claude OAuth wire、CL-N7/CL-N8、套餐投影及 receipt schema v1/revision 1 不变。末次检查之后的并发变化、跨 API 原子性和厂商接受性仍不由此证明。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，五份真实 operation receipt 与 thread/beta 接受性证据仍缺；当前工作树未提交，不用旧 committed-target observation 冒充本轮实现。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.89 Codex 五 operation 私有 receipt 最终内容复核

2026-10-08，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的提交态未变；另按 openai-docs 技能核对官方 [Responses WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)，本项不由此推断私有 OAuth 账号接受性或改动生产 wire。目标 `gpt_6_astra`、三个 GPT Image 2.5 exact-model operation 与 `ws_prewarm` 已在控制面请求前保存 receipt 原始字节和文件指纹，末次却只比较路径与元数据。五项各在首个 Account 请求时等长改写 receipt，并用测试预加载钩子让末次路径及描述符元数据读数保持原指纹；旧流程五例全部错误输出 `contract_verified/live_pending`。钩子仅验证“元数据不能代替内容”，不代表普通用户能恢复真实 `ctime`。

现在末次以 `O_NOFOLLOW` 只读描述符重新读取完整、至多 1 MiB 的原始 receipt 字节及 EOF，复核读前读后指纹与配置路径；五例均脱敏拒绝，正常五项、既有替换/原位改写、模型 manifest、绑定和时效 fixture 保持通过。Codex 专项 Node 80/80、全量 audit Node 712/712、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；生产 HTTP/WS/Image wire、receipt schema v1/revision 1、Images disabled 与 WS prewarm 未启用决策不变。末次检查后的并发变化、跨 API 原子性及厂商接受性仍不能由 fixture 证明。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，五份真实 receipt 和当前工作树提交态 observation 仍缺；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.90 Cursor OAuth/API Key 双 rail 私有 receipt 最终内容复核

2026-10-08，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的提交态未变；参考工作树中的未提交 Codex 文件继续排除，CUR-N2 仍为 `reviewed_no_wire_delta`。目标 OAuth、API Key 两条 rail 已在控制面请求前保存稳定 receipt 字节和指纹，末次只比路径和元数据。两条 rail 各在首个控制面请求时等长改写文件，用测试预加载钩子让末次路径及描述符元数据读数保持原指纹；旧流程两例均错误输出 `contract_verified/live_pending`。钩子的元数据伪装只证明指纹不能代替内容，不代表普通用户能够恢复真实 `ctime`。

现在最终验收以 `O_NOFOLLOW` 只读描述符重读完整、至多 1 MiB 的原始 receipt 字节和 EOF，并复核读前读后指纹及配置路径。两例均脱敏拒绝，正常双 rail、既有路径替换/原位改写、签名目录绑定、API Key 动态目录时效和末次绑定 fixture 保持通过。Cursor 专项 Node 57/57、全量 audit Node 715/715、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；生产 Cursor protobuf/session/目录 wire、OAuth/API Key 身份与 receipt schema v2/revision 2 不变。末次检查后的并发变化、跨 API 原子性以及真实厂商接受性仍不由 fixture 保证。两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，两份真实 receipt 与当前工作树提交态 observation 仍缺；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.91 Grok 三 operation 私有 receipt 最终内容复核

2026-10-08，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交 HEAD 未变；后者未提交的 Codex 文件继续排除，GR-N4 版本接受性仍需固定账号真实证据，本项不推断新 xAI wire。目标 inference、media、remote_compaction 三项私有 receipt 已在控制面请求前保存原始字节和文件指纹，但末次只比较路径与元数据。三项各在首个 Account 请求时等长改写文件，再用测试预加载钩子让末次路径及描述符元数据读取返回原指纹；旧流程三例均错误输出 `contract_verified/live_pending`。元数据伪装仅为对抗 fixture，不代表普通用户能恢复真实 `ctime`。

现在最终检查经 `O_NOFOLLOW` 只读描述符重读完整、至多 1 MiB 的原始 receipt 字节和 EOF，并复核读前读后指纹及配置路径；三例均脱敏拒绝，正常三项、既有文件替换/原位改写、目录/绑定/时效 fixture 保持通过。Grok 专项 Node 68/68、全量 audit Node 719/719、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；生产 Grok HTTP/WS/media/compaction wire、receipt schema v1/revision 1 和 GR-N4 决策不变。末次检查后的并发变化、跨 API 原子性与真实厂商接受性仍待证。三项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，三份真实 receipt、GR-N4 固定账号版本证据及当前工作树提交态 observation 仍缺；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.92 Kiro 八路私有 receipt 最终内容复核

2026-10-08，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的提交态未变；本项不推导新的 Kiro/Amazon Q wire。Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 八路私有 receipt 已在控制面请求前保存原始字节和文件指纹，末次却只比较路径与元数据。八个 scope 各在首个 Account 请求期间等长改写 receipt，并用测试预加载钩子让末次路径及描述符元数据读取返回原指纹；旧流程八例均错误输出 `contract_verified/live_pending`。钩子只验证元数据不能代替内容，不代表普通用户能够恢复真实 `ctime`。

现在最终检查经 `O_NOFOLLOW` 只读描述符重读完整、至多 1 MiB 的原始 receipt 字节及 EOF，复核读前读后指纹与配置路径；八例均脱敏拒绝，正常八路、既有文件替换/原位改写、双目录、绑定和时效 fixture 保持通过。Kiro 专项 Node 158/158、全量 audit Node 728/728、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；生产 Kiro/Amazon Q EventStream、auth kind/region 路由、KI-N5 usage authority、receipt schema v1/revision 1、Compact/shared cache disabled 决策不变。末次检查后的竞态、跨 API 原子性与厂商真实接受性仍待证。八个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，八份真实 receipt、KI-N5 专项证据及当前工作树提交态 observation 仍缺；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 业务源码仍受 `project-doc` 技能门禁。

### 18.93 Qoder 三 rail 私有 receipt 临时文件所有权复核

2026-10-09，Qoder 单类别接续。目标 Global OAuth、Global PAT、CN OAuth 的私有 receipt 发布器已核对最终硬链接内容和所属 inode，但临时文件仍是先用路径写入、失败时再无条件按路径删除。三条 rail 各在临时文件写完后以另一 inode 的哨兵文件替换该路径；旧流程虽拒绝发布，却均误删占位文件。现用 `O_CREAT|O_EXCL|O_NOFOLLOW` 排他创建临时文件，保留同一读写描述符完成写入和完整字节复核；失败清理仅在临时路径的 dev/inode 仍属于本次创建文件时 unlink。三例先红后绿，替换者文件及内容保持原样；正常三 rail、父目录交换、目录时效、目标排他发布和最终链接替换 fixture 保持通过。

本项只收紧私有证据清理边界，不改变生产 Qoder COSY wire、site/rail/Account/Provider/Share 选择、receipt schema v2/revision 2 或 `partial_live_verified/live_pending` 决策。`lstat` 与 unlink 仍有竞态，描述符锚定不构成跨 API/文件系统原子事务，fixture 不是厂商接受性。Qoder 专项 Node 83/83、全量 audit Node 728/728、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，三份真实 receipt 与登录/轮换、权威空目录、受控两段 401 证据仍缺；当前工作树未提交，不能用旧 committed-target observation 证明新逻辑。推荐 TokenRouter 要求本会话不可用的 `project-doc` 技能；读取其门禁文档的同一次命令误附带只读 `git status`、`rev-parse` 元数据检查，此后未读取业务文件或运行外仓 `--check-sources`，最新增量保持 `unreviewed_skill_gate`。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.94 CodeBuddy 双站 Share 唯一绑定复核

2026-10-09，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的提交态未变，两个未跟踪 `proxy.html`、`proxy.md` 继续排除；本项不引入新 CodeBuddy wire 推断。目标私有 receipt validator 原先把 Share 主字段拼进 `share.bindings`，再用 `some` 寻找每 App 正确条目；但生产 API 的 `bindings` 已包含主 App 条目，且 Share 导入不变量要求每 App 至多一条、主字段与对应条目一致。Intl/CN × Claude/Codex/Gemini 各添加一条冲突绑定，六个旧流程反例均错误输出 `contract_verified/live_pending`。初版校验会把真实主绑定误算成重复，已按实际 API 形状修正 fixture 与实现：直接要求原始 `bindings` 对三个 App 各有且仅有一条精确匹配，主字段也须与其唯一条目一致；额外两个主字段冲突反例拒绝。六个重复绑定反例先红后绿，正常双站与既有 receipt/目录/时效 fixture 保持通过，歧义 Share 在目录发网前即被拒绝。

本项只收紧私有验收证据，不改生产 CodeBuddy 转发、Share 路由、站点身份、receipt schema v2/revision 2 或 CB-N4 disabled 决策；控制面复核仍非跨 API 原子快照，fixture 不证明厂商接受性。CodeBuddy 专项 Node 62/62、全量 audit Node 738/738、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、CB-N4 CN 厂商目录/接受性及当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.95 Antigravity/AGY 双 rail Share 唯一绑定与启用状态复核

2026-10-09，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的提交态未变；本项不引入新 Google wire。目标双 rail 私有验收器原先把 Share 主字段拼进 `bindings`，再用 `some` 寻找 Claude/Gemini 正确条目，也未检查 enabled/status；然而生产 Share 投影的 `bindings` 已包含主 App，导入不变量要求每 App 至多一条且主字段一致。两条 rail 各构造 Claude/Gemini 重复冲突绑定、主字段冲突、disabled 与 paused 五种反例，旧流程十例全部错误输出 `contract_verified/live_pending`。现在按真实投影要求原始 `bindings` 对两 App 各有且仅有一条精确匹配，主字段与其唯一条目一致，并要求 `enabled=true/status=active`；十例先红后绿，正常双 rail 与既有 receipt、目录、漂移和时效 fixture 保持通过，歧义或停用 Share 在目录发网前拒绝。runbook 同步纠正了旧的“先查控制面再读 receipt”顺序描述。

本项只收紧私有验收证据，不改变生产 Antigravity/AGY wire、Share 路由、AG-N6 requestType、AG-D3/compaction、receipt schema v2/revision 2 或 compaction disabled 决策；控制面复核仍非跨 API 原子快照，fixture 不证明真实 Google 接受性。Antigravity 专项 Node 75/75、全量 audit Node 749/749、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6/AG-D3 专项接受性与当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.96 Claude 五 operation Share 唯一绑定与启用状态复核

2026-10-09，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的提交态未变；本项不推导新的 Anthropic wire。目标五项私有 receipt validator 原先把 Share 主字段拼进 `bindings`，再用 `some` 寻找正确 Claude Provider，且未检查 enabled/status；实际 Server Share 投影的 `bindings` 已包含主条目，导入不变量要求每 App 至多一条并与主字段一致。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 各添加重复冲突绑定、主字段冲突、disabled 与 paused 四种反例，旧流程二十例均错误输出 `contract_verified/live_pending`。现在要求原始 `bindings` 中恰好一条精确 Claude 绑定、主字段与对应唯一条目一致，且 `enabled=true/status=active`；二十例先红后绿，正常五项与既有凭据、计划、目录、文件和时效 fixture 保持通过，错误在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Claude OAuth wire、Share 路由、CL-N7/CL-N8、plan projection 或 receipt schema v1/revision 1；控制面复核不是跨 API 原子快照，fixture 不证明 Anthropic 接受性。Claude 专项 Node 126/126、全量 audit Node 770/770、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；五份真实 receipt、续线程/beta 厂商接受性和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.97 Codex 五 operation Share 唯一绑定与启用状态复核

2026-10-09，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的提交态未变；依 `openai-docs` 技能读取过官方 [Responses WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode)，本项不据此推断私有 OAuth entitlement 或修改 OpenAI wire。目标五项私有 receipt validator 原先把 Share 主字段拼进 `bindings`，再用 `some` 找正确 Codex Provider，且未检查 enabled/status；实际 Server Share 投影的 `bindings` 已包含主条目，导入不变量要求每 App 至多一条并与主字段一致。Astra、三个 GPT Image 2.5 变体和 WS prewarm 各构造重复冲突绑定、主字段冲突、disabled 和 paused 四种反例，旧流程二十例均错误输出 `contract_verified/live_pending`。现在要求原始 `bindings` 中恰好一条精确 Codex 绑定、主字段与对应唯一条目一致，且 `enabled=true/status=active`；二十例先红后绿，正常五项与既有 manifest、目录、文件、绑定和时效 fixture 保持通过，错误在模型目录或 manifest 发网前拒绝。

本项只收紧私有验收证据，不改变生产 Codex HTTP/WS/Image wire、Share 路由、receipt schema v1/revision 1、Images disabled 或 WS prewarm 未启用决策；控制面复核不是跨 API 原子快照，fixture 不证明真实 ChatGPT 账号权益。Codex 专项 Node 101/101、全量 audit Node 791/791、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；五份真实 receipt 和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.98 Cursor OAuth/API Key Share 唯一绑定与启用状态复核

2026-10-09，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的提交态参考保持冻结；其未提交 Codex 内容不作 Cursor 证据。本项不推导新的 Cursor wire。目标双 rail 私有 receipt validator 原先将 Share 主字段拼入 `bindings` 再用 `some` 找所选 Provider，也不检查 enabled/status；实际 Server Share 投影的 `bindings` 已包含主 App，导入不变量要求每 App 至多一条且主字段一致。OAuth/API Key 各构造重复冲突绑定、主字段冲突、disabled 与 paused 四种反例，旧流程八例全部错误输出 `contract_verified/live_pending`。现在直接要求原始 `bindings` 中所选 App 恰好一条精确绑定、主字段与其对应唯一条目一致，且 `enabled=true/status=active`；八例先红后绿，正常双 rail 及已有 receipt、漂移、时效和 API Key 动态目录 fixture 通过，错误均在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Cursor protobuf/session/目录 wire、Share 路由、OAuth/API Key 身份或 receipt schema v2/revision 2；控制面复核不是跨 API 原子快照，fixture 不证明真实账号接受性。Cursor 专项 Node 66/66、全量 audit Node 800/800、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt 和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.99 Grok 三 operation Share 唯一绑定与启用状态复核

2026-10-09，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交 HEAD 未变；后者未提交工作树内容继续排除。本项不推导新的 xAI wire。目标 inference、media、remote_compaction 三项私有 receipt validator 原先把 Share 主字段拼入 `bindings` 后用 `some` 找 Codex Provider，也不检查 enabled/status；实际 Server Share 投影的 `bindings` 已包含主条目，导入不变量要求每 App 至多一条并与主字段一致。三项各构造重复冲突绑定、主字段冲突、disabled 和 paused 四种反例，旧流程十二例全部错误输出 `contract_verified/live_pending`。现在直接要求原始 `bindings` 中 Codex 恰好一条精确 Grok 绑定、主字段与其对应唯一条目一致，且 `enabled=true/status=active`；十二例先红后绿，正常三项与已有 receipt、目录、漂移及时效 fixture 保持通过，错误均在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Grok HTTP/WS/media/compaction wire、Share 路由、receipt schema v1/revision 1、GR-N4 版本决策或 remote compaction disabled 状态；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。Grok 专项 Node 81/81、全量 audit Node 813/813、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。inference、media、remote_compaction 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 接受性和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.100 Kiro 八路 Share 唯一绑定与启用状态复核

2026-10-09，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交 HEAD 未变，参考工作树干净；本项不推导新的 Kiro/Amazon Q wire。目标八路私有 receipt validator 原先把 Share 主字段拼进 `bindings` 再用 `some` 找 Claude/Codex Provider，且未检查 enabled/status；旧 mock 还未模拟生产 Share 主字段与启用状态。Server Share 投影的 `bindings` 已包含主 App，导入不变量要求每 App 至多一条并与主字段一致。Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 各构造 Claude 重复冲突绑定、Codex 重复冲突绑定、主字段冲突、disabled 和 paused 五种反例，旧流程四十例全部错误输出 `contract_verified/live_pending`。现在按真实投影要求原始 `bindings` 对两 App 各恰好一条精确 Kiro 绑定、主字段与其对应唯一条目一致，且 `enabled=true/status=active`；四十例先红后绿，正常八路及已有 receipt、双目录、漂移和时效 fixture 保持通过，错误在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Kiro/Amazon Q EventStream、Share 路由、auth kind/region 选择、KI-N5 usage authority、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。Kiro 专项 Node 199/199、全量 audit Node 854/854、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。八个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`；八份真实 receipt、KI-N5 专项证据和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 业务源码仍受缺失的 `project-doc` 技能门禁。

### 18.101 Qoder 三 rail Share 唯一绑定与显式启用状态复核

2026-10-09，Qoder 单类别接续。推荐 TokenRouter 的 `AGENTS.md` 要求使用当前会话不可调用的 `project-doc` 技能，因此本轮停止于门禁说明，未读取业务源码、未运行外仓 `--check-sources`，最新业务增量继续 `unreviewed_skill_gate`。本项仅依据目标 Server 自身 Share 投影与导入不变量，不推导新 COSY wire。目标三 rail 私有 receipt 发布器原先将 Share 主字段拼入 `bindings` 再用 `some` 找 Claude/Codex/Gemini Provider；虽然能拒绝显式 disabled/paused，却把缺失 enabled/status 视为有效。生产 `bindings` 已包含主 App，且每 App 至多一条、主字段须与其唯一条目一致。Global OAuth、Global PAT、CN OAuth 各构造三 App 重复冲突绑定、主字段冲突、disabled、paused、缺 enabled、缺 status 八种反例：旧流程误接受其中十八例，已拒绝的六例保持拒绝。现在直接要求原始 `bindings` 对三 App 各恰好一条精确绑定、主字段一致，且 `enabled=true/status=active`；二十四例全部失败关闭且目录零请求、receipt 不发布，正常三 rail 及旧有目录、时效、排他发布、文件所有权和末次绑定 fixture 保持通过。

本项只收紧私有验收证据，不改变生产 Qoder COSY wire、Share 路由、site/rail 选择或 receipt schema v2/revision 2；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。Qoder 专项 Node 108/108、全量 audit Node 879/879、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。三条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt、登录/轮换、权威空目录、受控两段 401 和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.102 CodeBuddy 双站显式启用状态复核

2026-10-09，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD 未变，两个未跟踪 `proxy.html`、`proxy.md` 继续排除；本项不引入新 CodeBuddy wire。目标 Intl/CN 私有 receipt validator 已要求原始 Share `bindings` 对 Claude/Codex/Gemini 各唯一且精确、主字段一致，但启用状态只在 `enabled=false` 或存在且非 active 的 `status` 时拒绝，缺字段会被误当作有效。两站各构造缺 enabled、缺 status、disabled、paused 四种反例；缺字段四例旧流程错误输出 `contract_verified/live_pending`，显式 disabled/paused 四例原本就拒绝。现要求 `enabled=true/status=active`；八例均在目录发网前失败，正常双站以及既有 Share 绑定、目录、receipt 文件/时效和末次漂移 fixture 保持通过。

本项只收紧私有验收证据，不改变生产 CodeBuddy wire、Share 路由、Intl/CN 站点身份、receipt schema v2/revision 2 或 CB-N4 disabled 决策；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。CodeBuddy 专项 Node 71/71、全量 audit Node 888/888、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、CB-N4 CN 目录/接受性和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.103 Antigravity/AGY 双 rail 控制面 revision 作用域完整性复核

2026-10-09，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD 未变；本项不引入新 Google wire。目标双 rail 私有 receipt validator 的 Share 唯一性/启用状态已有门槛，但 Provider revision、runtime fingerprint 与 Share revision 作用域仍不完整：缺失 revision 会回退为 `0`，空 fingerprint 可以进入 binding digest。两条 rail 各构造缺 Claude revision、负 Gemini revision、空 Claude/Gemini fingerprint、缺 Share revision、负 Share revision 六类反例，receipt 也按该不完整作用域自洽生成；旧流程十二例全部错误输出 `contract_verified/live_pending`。现在要求控制面明确提供两个 Provider 的非负安全整数 `providerRevision`、非空字符串 `runtimeFingerprint` 与 Share 的非负安全整数 `configRevision`，不再用 fallback 补齐证据作用域；十二例先红后绿，正常双 rail 与已有 receipt 文件、Share、目录、时效和末次漂移 fixture 保持通过，错误在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Antigravity/AGY wire、Share 路由、AG-N6 requestType、AG-D3/compaction、receipt schema v2/revision 2 或 compaction disabled 决策；控制面复核不是跨 API 原子快照，fixture 不证明 Google 接受性。Antigravity 专项 Node 88/88、全量 audit Node 901/901、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6/AG-D3 专项接受性与当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.104 Claude 五 operation 控制面 revision/fingerprint 作用域完整性复核

2026-10-09，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD 未变；本项不推导新的 Anthropic wire。目标五项私有 receipt validator 的 Share 唯一性/启用状态已有门槛，但 Provider/Share revision 缺字段时会回退为 `0`，只含空格的 runtime fingerprint 也会进入 scope/binding digest。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 各构造缺 Provider revision、缺 Share revision、空白 fingerprint 三种自洽 receipt 反例，旧流程十五例全部错误输出 `contract_verified/live_pending`。现在直接要求控制面明确提供非负安全整数 `providerRevision`、`configRevision` 与非空白字符串 `runtimeFingerprint`，不再用 fallback 补齐证据作用域；十五例先红后绿，正常五项与既有 plan、Share、receipt 文件/时效和末次绑定 fixture 保持通过，错误在模型目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Claude OAuth wire、Share 路由、CL-N7/CL-N8、plan projection 或 receipt schema v1/revision 1；控制面复核不是跨 API 原子快照，fixture 不证明 Anthropic 接受性。Claude 专项 Node 142/142、全量 audit Node 917/917、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；五份真实 receipt、续线程/beta 厂商接受性和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.105 Codex 五 operation 控制面 revision/fingerprint 作用域完整性复核

2026-10-09，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交状态未变；官方 Responses WebSocket 文档不证明私有 OAuth 权益，本项不推导新的 OpenAI wire。五项私有 receipt validator 虽已校验 Share 唯一性与启用状态，但缺失 Provider/Share revision 会回退为 `0`，空白 runtime fingerprint 也会进入 scope/binding digest。`gpt_6_astra`、三个 GPT Image 2.5 exact-model operation 与 `ws_prewarm` 各构造缺 Provider revision、缺 Share revision、空白 fingerprint 三种自洽 receipt 反例，旧流程十五例全部错误输出 `contract_verified/live_pending`。现在要求控制面明确提供非负安全整数 `providerRevision`、`configRevision` 与非空白字符串 `runtimeFingerprint`，不再用 fallback 补齐证据作用域；十五例先红后绿，正常五项及既有 manifest、Share、receipt 文件/时效和末次漂移 fixture 保持通过，错误在模型目录或 manifest 发网前拒绝。

本项只收紧私有验收证据，不改变生产 Codex HTTP/WS/Image wire、Share 路由、receipt schema v1/revision 1、Images disabled 或 WS prewarm 未启用决策；控制面复核不是跨 API 原子快照，fixture 不证明真实 ChatGPT 账号权益。Codex 专项 Node 117/117、全量 audit Node 933/933、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；五份真实 receipt 和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.106 Cursor OAuth/API Key 双 rail 控制面 revision/fingerprint 作用域完整性复核

2026-10-09，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` / tree `8d1c312f61d115767c9478416cb039750eaf113a` 的已提交状态不变；参考工作树 22 项未提交 Codex 内容继续排除，CUR-N2 保持 `reviewed_no_wire_delta`，本项不引入新的 Cursor wire。目标双 rail 私有 receipt validator 的 Share 唯一绑定与启用状态已有门槛，但 Provider 投影同时拥有顶层 `revision` 和 runtime `providerRevision`，旧流程在任一缺失时取另一份、两份冲突时优先 runtime；双缺失或 Share revision 缺失回退为 `0`，空白 fingerprint 也进入 scope/binding digest。OAuth/API Key 各构造缺 runtime revision、缺顶层 revision、两份不一致、两份均缺、缺 Share revision、空白 fingerprint 六种自洽反例，旧流程十二例全部错误输出 `contract_verified/live_pending`。现要求两份 Provider revision 均为非负安全整数且相等、Share `configRevision` 明确为非负安全整数、`runtimeFingerprint` 是非空白字符串；十二例先红后绿，正常双 rail 及既有 Share、目录时效、receipt 文件和末次绑定 fixture 通过，错误在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Cursor protobuf/session/目录 wire、OAuth/API Key 身份、Share 路由或 receipt schema v2/revision 2；多次控制面读取不是原子快照，fixture 不证明真实厂商接受性。Cursor 专项 Node 79/79、全量 audit Node 946/946、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt 和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.107 Grok 三 operation 控制面真实投影与 revision/fingerprint 作用域完整性复核

2026-10-09，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态未变；后者六项未提交 Codex 内容继续排除，本项不引入新的 xAI wire。目标 Server `ProviderView` 实际输出顶层 `revision` 与 runtime `providerRevision`，但旧 Grok mock 给了不存在的顶层 `providerRevision`，掩盖了真实控制面正常路径：mock 改为真实形状后，inference、media、remote_compaction 三个正常 fixture 先全部失败。旧 validator 在读不到虚构字段时回退为 `0`，即使真实 Provider revision 已存在，receipt scope 也可能错误绑定为零。三项各构造缺 runtime revision、缺顶层 revision、两份不一致、两份均缺、仅有旧式虚构顶层字段、缺 Share revision 与空白 fingerprint 七种自洽反例，旧流程二十一例全部错误输出 `contract_verified/live_pending`。现直接要求两份真实 Provider revision 均为非负安全整数且一致、Share `configRevision` 明确为非负安全整数、runtime fingerprint 为非空白字符串；正常三项恢复通过，二十一例先红后绿，既有 Share、目录、receipt 文件与末次漂移 fixture 保持通过，错误均在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Grok HTTP/WS/media/compaction wire、Share 路由、receipt schema v1/revision 1、GR-N4 版本决策或 remote compaction disabled 状态；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。Grok 专项 Node 103/103、全量 audit Node 968/968、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试与文档，未重跑无变化的全量 Rust 套件。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 接受性和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.108 Kiro 八路控制面真实投影与双 Provider revision/fingerprint 完整性复核

2026-10-09，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交 HEAD/tree 未变，参考工作树干净；本项不推导新的 Kiro/Amazon Q wire。目标 Server `ProviderView` 实际输出顶层 `revision` 与 runtime `providerRevision`，旧 Kiro mock 却给了不存在的顶层 `providerRevision`，掩盖真实控制面正常路径：mock 改成真实形状后，八个 auth-kind×region 正常 fixture 先全部失败。旧 validator 在找不到虚构字段时回退为 `0`，即使真实 Provider revision 存在，receipt scope 也可能错误绑定为零。Builder ID、IdC、Social、API Key × 两区域各构造 Claude/Codex 两个 Provider 分别缺顶层 revision、缺 runtime revision、两份不一致、仅有旧式虚构顶层字段、空白 fingerprint，另构造缺 Share revision，共十一种自洽反例；旧流程八十八例全部错误输出 `contract_verified/live_pending`。现在要求两个 Provider 各自的顶层/runtime revision 都是非负安全整数且相等、两个 runtime fingerprint 均为非空白字符串、Share `configRevision` 明确为非负安全整数；八路正常 fixture 恢复通过，八十八例先红后绿，已有 Share、双目录、receipt 文件/时效和末次绑定 fixture 保持通过，错误在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Kiro/Amazon Q EventStream、auth kind/region 选择、Share 路由、KI-N5 usage authority、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。Kiro 专项 Node 288/288、全量 audit Node 1057/1057、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。八个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`；八份真实 receipt、KI-N5 专项证据和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 业务源码仍受缺失的 `project-doc` 技能门禁。

### 18.109 Qoder 三 rail 控制面真实投影与三 Surface revision/fingerprint 完整性复核

2026-10-09，Qoder 单类别接续。推荐 TokenRouter 的 `AGENTS.md` 要求当前会话不可调用的 `project-doc` 技能，因此未读取业务源码或运行外仓 `--check-sources`，最新业务增量继续 `unreviewed_skill_gate`；本项只依据目标 Server 自身控制面合同，不引入新 COSY wire。实际 `ProviderView` 同时输出顶层 `revision` 与 runtime `providerRevision`，旧 Qoder mock 却给了不存在的顶层 `providerRevision`，掩盖真实控制面正常路径：mock 改成真实形状后，Global OAuth、Global PAT、CN OAuth 三条正常 fixture 先全部失败。旧发布器读不到虚构字段时回退为 `0`，可将三个 Surface 的真实 Provider revision 错误写入 receipt。三 rail 各构造 Claude/Codex/Gemini 的缺顶层 revision、缺 runtime revision、两份不一致、仅有旧式虚构字段、空白 fingerprint，另构造缺 Share revision，共十六类反例；旧流程四十八例全部未能按预期拒绝。现要求三个 Provider 各自顶层/runtime revision 是非负安全整数且相等、三个 runtime fingerprint 为非空白字符串、Share `configRevision` 明确为非负安全整数；正常三 rail 恢复通过，四十八例先红后绿，已有 Share、目录、receipt 排他发布、文件所有权与末次绑定 fixture 保持通过，错误在三份目录发网前拒绝且不落盘。

本项只收紧私有验收证据，不改变生产 COSY wire、site/rail、Share 路由或 receipt schema v2/revision 2；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。Qoder 专项 Node 157/157、全量 audit Node 1106/1106、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过；受门禁约束的外仓 `--check-sources` 未运行。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。三条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt、登录/轮换、权威空目录、受控两段 401 与当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.110 CodeBuddy Intl/CN 双站控制面真实投影与三 Surface revision/fingerprint 完整性复核

2026-10-09，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD/tree 未变，未跟踪 `proxy.html`、`proxy.md` 继续排除；本项不推导新的 CodeBuddy wire。目标 Server `ProviderView` 实际输出顶层 `revision` 与 runtime `providerRevision`，旧 CodeBuddy mock 却给不存在的顶层 `providerRevision`，掩盖真实控制面正常路径：mock 改成真实形状后，Intl/CN 两个正常 fixture 先失败。旧 validator 读不到虚构字段时回退为 `0`，可将三个 Surface 的真实 Provider revision 错误绑定进 receipt。两站各构造 Claude/Codex/Gemini 的缺顶层 revision、缺 runtime revision、两份不一致、仅有旧式虚构顶层字段、空白 fingerprint，另构造缺 Share revision，共十六类自洽反例；旧流程三十二例全部未按预期拒绝。现在要求三个 Provider 各自顶层/runtime revision 是非负安全整数且相等、三个 runtime fingerprint 为非空白字符串、Share `configRevision` 明确为非负安全整数；正常双站恢复通过，三十二例先红后绿，已有 Share、三目录、receipt 文件/时效和末次绑定 fixture 保持通过，错误均在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 CodeBuddy wire、Intl/CN 身份、Share 路由、receipt schema v2/revision 2 或 CB-N4 disabled 决策；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。CodeBuddy 专项 Node 104/104、全量 audit Node 1139/1139、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、CB-N4 CN 目录/接受性和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.111 Antigravity/AGY 双 rail 控制面真实投影与双 Provider revision 一致性复核

2026-10-09，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD 未变；本项不引入新 Google wire。目标 Server `ProviderView` 实际为顶层 `revision` 与 runtime `providerRevision`，旧 Antigravity mock 却只给不存在的顶层 `providerRevision`；将 mock 改成真实形状后，`antigravity_oauth`、`agy_oauth` 两条正常 fixture 先全部失败。旧 validator 虽已拒绝缺失 revision、负 revision 与空白 fingerprint，却仍读取虚构字段。双 rail × Claude/Gemini 各构造仅有旧式顶层字段、缺真实顶层 revision、缺 runtime revision、两份不一致四种自洽反例，且非目标 App 也保留旧字段以排除旁路失败；旧流程十六例均错误输出 `contract_verified/live_pending`。现在要求两个 Provider 各自的真实顶层/runtime revision 明确为非负安全整数且相等；正常双 rail 按真实投影恢复通过，十六例先红后绿，既有 Share、目录、receipt 文件/时效、revision 缺失/负数与末次绑定 fixture 保持通过，错误在目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Antigravity/AGY wire、Share 路由、AG-N6 requestType、AG-D3/compaction、receipt schema v2/revision 2 或 compaction disabled 决策；控制面复核不是跨 API 原子快照，fixture 不证明 Google 接受性。Antigravity 专项 Node 105/105、全量 audit Node 1156/1156、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6/AG-D3 专项证据和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.112 Claude 五 operation 控制面真实投影与 Provider revision 一致性复核

2026-10-09，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD 未变；本项不推导新的 Anthropic wire。目标 Server `ProviderView` 实际输出顶层 `revision` 与 runtime `providerRevision`，旧 Claude mock 却给不存在的顶层 `providerRevision`；mock 改为真实形状后，`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 五项正常 fixture 先全部失败。旧 validator 虽已拒绝缺 Provider/Share revision 与空白 fingerprint，却仍只读虚构字段。五项各构造仅有旧式顶层字段、缺真实顶层 revision、缺 runtime revision、两份不一致四种自洽反例，旧流程二十例全部错误输出 `contract_verified/live_pending`。现在要求真实顶层/runtime Provider revision 均为非负安全整数且相等；五项正常 fixture 恢复通过，二十例先红后绿，已有 plan、Share、receipt 文件/时效和末次漂移 fixture 保持通过，错误在模型目录发网前拒绝。

本项只收紧私有验收证据，不改变生产 Claude OAuth wire、Share 路由、CL-N7/CL-N8、plan projection 或 receipt schema v1/revision 1；控制面复核不是跨 API 原子快照，fixture 不证明 Anthropic 接受性。Claude 专项 Node 163/163、全量 audit Node 1177/1177、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；五份真实 receipt、续线程/beta 厂商接受性和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.113 Codex 五 operation 控制面真实投影与 Provider revision 一致性复核

2026-10-09，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交 HEAD 未变；本项只根据目标 Server 自身控制面合同修正私有验收，不推导新 OpenAI wire 或账号权益。真实 `ProviderView` 为顶层 `revision` 与 runtime `providerRevision`，旧 Codex mock 却给不存在的顶层 `providerRevision`；mock 改成真实形状后，Astra、三个 Image 2.5 变体与 WS prewarm 五项正常 fixture 先全部失败。旧 validator 虽已拒绝缺 Provider/Share revision 与空白 fingerprint，却仍只读取虚构字段。五项各构造仅有旧式顶层字段、缺真实顶层 revision、缺 runtime revision、两份不一致四种自洽反例，旧流程二十例全部错误输出 `contract_verified/live_pending`。现在要求真实顶层/runtime Provider revision 明确为非负安全整数且相等；正常五项恢复通过，二十例先红后绿，既有 Share、manifest、receipt 文件/时效和末次漂移 fixture 保持通过，错误在模型目录或 manifest 发网前拒绝。

本项只收紧私有验收证据，不改变生产 Codex HTTP/WS/Image wire、Share 路由、receipt schema v1/revision 1、Images disabled 或 WS prewarm 未启用决策；控制面复核不是跨 API 原子快照，fixture 不证明真实 ChatGPT 账号权益。Codex 专项 Node 138/138、全量 audit Node 1198/1198、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；五份真实 receipt 和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.114 Cursor OAuth Account 布尔投影完整性复核

2026-10-09，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` / tree `8d1c312f61d115767c9478416cb039750eaf113a` 的已提交状态未变；当前未提交的 Codex 工作树内容继续排除，CUR-N2 仍为 `reviewed_no_wire_delta`。本项根据目标 Server 自身 `AccountPublicView` 合同审计私有验收器，不引入新 Cursor wire。真实 OAuth Account 投影必带 `hasApiKey` 和 `needsRelogin` 布尔字段，旧 validator 只在它们为 `true` 时拒绝；两个字段各自缺失的反例先错误输出 `contract_verified/live_pending`。现在要求两者都明确为 `false`，两例先红后绿，均在 Provider 和模型目录请求前脱敏失败，正常 OAuth 与 API Key fixture 保持通过。

本项只收紧 OAuth Account 证据完整性，不改变生产 Cursor protobuf/session/目录 wire、OAuth/API Key 身份、Share 路由或 receipt schema v2/revision 2；控制面复核不是跨 API 原子快照，fixture 不证明真实账号接受性。Cursor 专项 Node 82/82、全量 audit Node 1201/1201、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt 与当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.115 Grok 三 operation OAuth Account 布尔投影完整性复核

2026-10-09，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交 HEAD 未变；后者六项未提交 Codex 文件继续排除。本项只根据目标 Server 自身 `AccountPublicView` 合同审计私有 receipt validator，不推导新 xAI wire。真实 OAuth Account 投影必带布尔 `hasApiKey` 和 `needsRelogin`，旧 validator 只在它们为 `true` 时拒绝；inference、media、remote_compaction 各自构造两个字段分别缺失、为 `null`、为字符串 `"false"` 的六种反例，旧流程十八例全部错误输出 `contract_verified/live_pending`。现在要求两字段明确为布尔 `false`，十八例先红后绿，均在 Provider 和目录请求前脱敏失败，正常三项 fixture 保持通过。

本项只收紧私有验收证据，不改变生产 Grok HTTP/WS/media/compaction wire、账号与 Share 路由、receipt schema v1/revision 1、GR-N4 版本决策或 remote compaction disabled 状态；控制面复核不是跨 API 原子快照，fixture 不证明真实 xAI 账号接受性。Grok 专项 Node 122/122、全量 audit Node 1220/1220、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 证据与当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.116 Kiro 八路 Account relogin 布尔投影完整性复核

2026-10-09，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交 HEAD/tree 未变、参考工作树干净；本项只根据目标 Server 自身 `AccountPublicView` 合同审计私有 receipt validator，不推导新 Kiro/Amazon Q wire。真实 Account 投影必带布尔 `needsRelogin`，旧 Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 八路 validator 只在值为 `true` 时拒绝；各路分别构造字段缺失、`null`、字符串 `"false"` 三种反例，旧流程二十四例全部错误输出 `contract_verified/live_pending`。现在要求明确的布尔 `false`，二十四例先红后绿，均在 Provider 和双目录请求前脱敏失败，正常八路 fixture 保持通过。

本项只收紧私有验收证据，不改变生产 Kiro/Amazon Q EventStream、auth kind/region 与 Share 路由、KI-N5 usage authority、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策；控制面复核不是跨 API 原子快照，fixture 不证明真实厂商接受性。Kiro 专项 Node 313/313、全量 audit Node 1245/1245、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。八个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；八份真实 receipt、KI-N5 专项证据与当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 业务源码仍受当前会话缺失的 `project-doc` 技能门禁。

### 18.117 Qoder 三 rail Account 凭据布尔投影完整性复核

2026-10-09，Qoder 单类别接续。推荐 TokenRouter 的 `AGENTS.md` 要求当前会话不可调用的 `project-doc` 技能；本项只读取其门禁说明，未读取业务源码或运行外仓 `--check-sources`，最新业务增量继续 `unreviewed_skill_gate`。本项依据目标 Server 自身必填 `AccountPublicView` 字段，不推导新 COSY wire。旧私有 receipt 发布器对 Global PAT 的 `hasAccessToken/hasRefreshToken`、Global/CN OAuth 的 `hasApiKey` 用真假值判断，且三 rail 的 `needsRelogin` 只在 `true` 时拒绝；三 rail 共二十一种缺字段、`null`、字符串 `"false"` 反例中，十七例错误进入后续验收，四例字符串凭据反例原已拒绝。现在要求 PAT 的 `hasApiKey=true/hasAccessToken=false/hasRefreshToken=false`、OAuth 的 `hasApiKey=false/hasAccessToken=true/hasRefreshToken=true`，以及三 rail 的 `needsRelogin=false` 均为明确布尔值；二十一例全部在 Provider 与目录请求前失败且不落盘，正常三 rail fixture 保持通过。

本项只收紧私有验收证据，不改变生产 COSY wire、site/rail、Account/Provider/Share 路由或 receipt schema v2/revision 2；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。Qoder 专项 Node 179/179、全量 audit Node 1267/1267、smoke Node 13/13；Qoder CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过；受门禁约束的外仓 `--check-sources` 未运行。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt、登录/轮换、权威空目录、受控两段 401 与当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.118 CodeBuddy Intl/CN Account 布尔投影完整性复核

2026-10-09，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD/tree 未变，未跟踪的 `proxy.html`、`proxy.md` 继续排除；本项只依据目标 Server 自身 `AccountPublicView` 合同，不推导新 CodeBuddy wire。真实 Account 投影必带布尔 `hasApiKey` 与 `needsRelogin`，旧 Intl/CN 私有 receipt validator 只在值为 `true` 时拒绝；两站各构造两字段分别缺失、为 `null`、为字符串 `"false"` 的六种反例，共十二例均错误输出 `contract_verified/live_pending`。现在要求两字段明确为布尔 `false`，十二例先红后绿，均在 Provider 与三份目录请求前脱敏失败，正常双站 fixture 保持通过。

本项只收紧私有验收证据，不改变生产 CodeBuddy wire、Intl/CN 站点身份、Share 路由、receipt schema v2/revision 2 或 CB-N4 disabled 决策；控制面复核不是跨 API 原子快照，fixture 不证明真实账号接受性。CodeBuddy 专项 Node 117/117、全量 audit Node 1280/1280、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、CB-N4 国内站目录/接受性和当前工作树提交态 observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.119 Antigravity/AGY OAuth Account 布尔投影完整性复核

2026-10-09，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD 未变。本项只依据目标 Server 自身 `AccountPublicView` 合同修正私有 receipt validator，不推导新 Google wire。真实 OAuth Account 投影必带布尔 `hasApiKey` 和 `needsRelogin`，旧 Antigravity/AGY 双 rail validator 却只在值为 `true` 时拒绝。两 rail 各构造两个字段分别缺失、为 `null`、为字符串 `"false"` 的六种反例，共十二例旧流程全部错误输出 `contract_verified/live_pending`。现在要求两个字段明确为布尔 `false`；十二例先红后绿，在 Provider 与双目录请求前脱敏拒绝，正常双 rail fixture 保持通过。

本项只收紧私有验收证据，不改变生产 Antigravity/AGY wire、Share 路由、AG-N6 requestType、AG-D3/compaction、目录缓存或 receipt schema v2/revision 2；控制面复核不是跨 API 原子快照，fixture 不证明真实 Google 接受性。Antigravity 专项 Node 118/118、全量 audit Node 1293/1293、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。`antigravity_oauth`、`agy_oauth` 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6/AG-D3 专项接受性与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.120 Claude 五 operation OAuth Account 布尔投影完整性复核

2026-10-09，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD 未变；本项只根据目标 Server 自身 `AccountPublicView` 合同修正私有 receipt validator，不推导新 Anthropic wire。真实 OAuth Account 投影必带布尔 `hasApiKey` 和 `needsRelogin`，旧五项 validator 却只在它们为 `true` 时拒绝。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 各构造两个字段分别缺失、为 `null`、为字符串 `"false"` 的六种反例，共三十例旧流程均错误输出 `contract_verified/live_pending`。现在要求两字段明确为布尔 `false`，三十例先红后绿，在 Provider 与模型目录请求前脱敏拒绝，正常五项及 Opus 5.5 access-only 正例保持通过。

本项只收紧私有验收证据，不改变生产 Claude OAuth wire、Share 路由、CL-N7/CL-N8、canonical plan 或 receipt schema v1/revision 1；控制面复核不是跨 API 原子快照，fixture 不证明真实 Anthropic 接受性。Claude 专项 Node 194/194、全量 audit Node 1324/1324、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；五份真实 receipt、续线程/beta 厂商接受性和当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.121 Codex 五 operation OAuth Account 布尔投影完整性复核

2026-10-09，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交 HEAD 未变。依 `openai-docs` 技能核对官方 [Create a model response](https://developers.openai.com/api/reference/resources/responses/methods/create)，但公开 Responses API 不证明私有 OAuth 权益；本项仅依据目标 Server 自身 `AccountPublicView` 合同修正私有 receipt validator，不推导新的 OpenAI wire。真实 OAuth Account 投影必带布尔 `hasApiKey` 和 `needsRelogin`，旧 Astra、三个 GPT Image 2.5 变体及 WS prewarm 五项 validator 却只在它们为 `true` 时拒绝。五项各构造两字段分别缺失、为 `null`、为字符串 `"false"` 的六种反例，共三十例旧流程错误输出 `contract_verified/live_pending`。现在要求两个字段明确为布尔 `false`，三十例先红后绿，在 Provider 和模型目录/manifest 请求前脱敏拒绝，正常五项 fixture 保持通过。

本项只收紧私有验收证据，不改变生产 Codex HTTP/WS/Image wire、Share 路由、receipt schema v1/revision 1、Images disabled 或 WS prewarm 未启用决策；控制面复核不是跨 API 原子快照，fixture 不证明真实 ChatGPT 账号权益。Codex 专项 Node 169/169、全量 audit Node 1355/1355、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；五份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.122 Cursor API Key 动态目录接受前再验证

2026-10-09，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` / tree `8d1c312f61d115767c9478416cb039750eaf113a` 的已提交状态未变；22 项未提交 Codex 内容继续排除，CUR-N2 保持 `reviewed_no_wire_delta`。目标 API Key rail 的模型目录是当前 Provider 静态凭据作用域下的动态 `cursor_public_api` 结果，旧私有 receipt validator 却只在初次读取时检查精确 `*-fast` 模型、来源、stale 与新鲜度。随后 receipt 和末次 Provider/Share 复核期间模型被撤下或目录权威改变，仍可能接受旧证据。回环服务让第一次目录有效，第二次分别撤下模型、标记 stale、把来源改为静态 registry；旧流程三例只查一次并错误输出 `contract_verified/live_pending`。现在仅 API Key rail 在末次绑定后经同一签名 Share/Provider 重新读目录，要求所选模型仍唯一且来源、stale、新鲜度合格，并在共同接受时刻复核两次目录时间。三例先红后绿；正常 API Key fixture 两次请求、OAuth 静态别名 fixture 一次请求均通过。

本项只收紧私有动态目录证据，不改变生产 Cursor protobuf/session/目录发布 wire、OAuth/API Key 身份、Share 路由或 receipt schema v2/revision 2；分次读取不是跨控制面/上游原子快照，也不证明真实厂商 entitlement。Cursor 专项 Node 86/86、全量 audit Node 1359/1359、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/provider/cursor.md` 已同步接受门槛。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt 和当前工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.123 Grok 三 operation 动态目录接受前内容复核

2026-10-09，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态未变；后者六项未提交 Codex 内容排除。GR-N4 的同账号版本/header 接受性继续 `live_pending_no_wire_change`，本项不引入新 xAI wire。目标 inference、media、remote_compaction 的 receipt 已绑定初次完整目录摘要，但旧私有 validator 在 receipt 和末次 Account/Provider/Share 复核后只查旧时间戳，目录若撤下模型、增加模型或变成 stale/伪来源仍可接受。三项各构造第二次目录的模型撤下、添加、stale、伪来源、超前时间五类反例；旧流程十五例各只读一次并错误输出 `contract_verified/live_pending`。现在末次绑定后经同一签名 Share/Provider 再读目录，要求当前模型唯一、来源合法、非 stale、时间新鲜且完整 `data` 摘要与 receipt 绑定的初次摘要相同；两次时间戳在共同接受时刻复核。十五例先红后绿，三项 `upstream`→`cache_fresh` 合法来源切换的正例也通过，不要求来源字面不变。

本项只收紧私有目录证据，不改变生产 Grok HTTP/WS/media/compaction wire、目录缓存、默认 CLI 版本、group header、receipt schema v1/revision 1 或 remote compaction disabled 决策；分次读取不是跨控制面/上游原子快照，fixture 不证明真实 xAI entitlement。Grok 专项 Node 141/141、全量 audit Node 1378/1378、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/provider/grok-oauth.md` 已同步验收门槛。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 接受性和当前工作树 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.124 Kiro 八路 Claude/Codex 双目录接受前内容复核

2026-10-09，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交 HEAD/tree 未变、参考工作树干净；KI-N5 context usage 继续只作 shadow，本项不推导新 EventStream wire。八个 auth-kind×region 私有 receipt 已分别绑定 Claude、Codex 的初次完整目录摘要，旧 validator 在 receipt 与末次 Account/双 Provider/Share 复核后却只重查旧时间戳。八路 × 两 App 各构造末次目录撤下模型、添加模型、标记 stale、改为伪来源或时间超前五类反例，共八十例旧流程只读首轮双目录并错误输出 `contract_verified/live_pending`。现在末次绑定后通过同一签名 Share/对应 Provider 再读双目录，要求两 App 各自模型唯一、来源受限、非 stale、四份时间戳在共同接受时刻新鲜，且当前完整 `data` 摘要等于 receipt 绑定的初次摘要。八十例先红后绿；八路 × 两 App 共十六个合法来源切换正例也通过。Claude 末次目录已失败时立即拒绝，不强求再请求 Codex。

本项只收紧私有目录证据，不改变生产 Kiro/Amazon Q EventStream、目录 TTL、auth kind/region 路由、KI-N5 计量权威、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策；分次读取不是跨控制面/上游原子快照，fixture 不证明厂商接受性。Kiro 专项 Node 410/410、全量 audit Node 1475/1475、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/provider/kiro-prompt-cache.md` 已同步双目录门槛。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。八个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；八份真实 receipt、KI-N5 专项证据及当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其最新 TokenRouter 业务源码仍受当前会话缺失的 `project-doc` 技能门禁。

### 18.125 Qoder 三 rail 三目录接受前内容复核

2026-10-09，Qoder 单类别接续。推荐 TokenRouter 的 `AGENTS.md` 仍要求本会话不可用的 `project-doc` 技能；没有读取其业务源码或运行外仓 `--check-sources`，最新参考增量继续 `unreviewed_skill_gate`。本项仅核对目标 Server 的私有 receipt 验收：Global OAuth、Global PAT、CN OAuth 的 Claude/Codex/Gemini 三份目录已经绑定初次完整 `data` 摘要，但旧 validator 在末次 Account/三 Provider/Share 复核后只查旧时间戳。三 rail × 三 App 各构造第二次目录撤下模型、添加模型、stale、伪来源、超前时间五类反例，共四十五例旧流程都只请求首轮目录并错误发布 `contract_verified/live_pending`。现在末次绑定后经同一签名 Share 与对应 Provider 栅栏重读三份目录，要求选定模型仍存在、来源合法、非 stale、当前完整 `data` 摘要与 receipt 初次摘要各自相同；初次和末次共六份时间戳在同一接受时刻复核。四十五例先红后绿，正常三 rail fixture 保持通过。

本项只提高私有目录证据门槛，不改变生产 COSY wire、site/rail/Account 路由、目录 TTL、receipt schema v2/revision 2 或 `live_pending` 决策；分次读取不是跨控制面/上游原子快照，fixture 不能证明真实厂商 entitlement。Qoder 专项 Node 225/225；自包含 Qoder reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/provider/qoder-cosy.md` 已同步双轮三目录门槛。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。三份真实 receipt、登录/轮换、权威空目录、受控两段 401 与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

收尾复核：全量 Node audit 1521/1521、smoke Node 13/13，本地 server smoke 通过；三条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。外仓 `--check-sources` 仍因技能门禁未运行，不能把本轮自包含核验当作最新 TokenRouter 源码对比。

### 18.126 CodeBuddy Intl/CN 三目录接受前内容复核

2026-10-09，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD/tree 未变，未跟踪的 `proxy.html`、`proxy.md` 继续排除。只读核对其 WorkBuddy `/v3/config` 产品目录：上游失败不使用静态回退，disabled 与 CLI agent allowlist 参与可用模型筛选；本项不推导新 CodeBuddy wire。目标 Intl/CN 私有 receipt 已绑定 Claude/Codex/Gemini 三份初次完整目录摘要，但旧 validator 在末次 Account/三 Provider/Share 复核后只查初次时间戳。两站 × 三 App 分别构造第二次目录撤下选定模型、添加模型、stale、伪来源、超前时间五类反例，共三十例旧流程均只读首轮目录并错误输出 `contract_verified/live_pending`。现在末次绑定后通过同一签名 Share 与各 App Provider 栅栏重读三份目录，要求合法来源、非 stale、选定模型恰好一次，且当前完整 `data` 摘要与 receipt 绑定的初次摘要分别相同；初次和末次共六份时间戳在共同接受时刻新鲜。三十例先红后绿，正常双站 fixture 保持通过。

本项只提高私有目录证据门槛，不改变生产 CodeBuddy wire、Intl/CN 身份、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled 决策；分次读取不是跨控制面/厂商原子快照，fixture 不证明真实权益。CodeBuddy 专项 Node 148/148、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/provider/codebuddy-oauth.md` 已同步双轮三目录门槛。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。Intl/CN 两份真实 receipt、CB-N4 CN 厂商目录/接受性及当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

收尾复核：全量 Node audit 1552/1552；Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`。本项并未生成或验收真实厂商 receipt。

### 18.127 Antigravity/AGY 双 rail Claude/Gemini 目录接受前内容复核

2026-10-09，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD/tree 未变，本项不推导新 Google wire。目标 `antigravity_oauth`、`agy_oauth` 的私有 receipt 已分别绑定 Claude/Gemini 两份初次完整目录摘要，但旧 validator 在末次 Account/双 Provider/Share 复核后只查旧时间戳。双 rail × 两 App 各构造第二次目录撤下所选模型、添加模型、stale、伪来源、超前时间五类反例，共二十例旧流程只读首轮双目录并错误输出 `contract_verified/live_pending`。现在末次绑定后通过同一签名 Share 与对应 Provider 栅栏重读双目录，要求权威来源、非 stale、选定模型恰好一次，当前完整 `data` 摘要各自等于 receipt 的初次摘要；初次与末次共四份时间戳在共同接受时刻新鲜。二十例先红后绿，正常双 rail fixture 保持通过。

本项只收紧私有目录证据，不改变生产 Antigravity/AGY wire、Share 路由、AG-N6 requestType、AG-D3/compaction、目录 TTL、receipt schema v2/revision 2 或 compaction disabled 决策；分次读取不是跨控制面/上游原子快照，fixture 不证明 Google 账号接受性。Antigravity 专项 Node 139/139、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/acceptance/real-acceptance-runbook.md` 已同步双轮双目录门槛。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；真实 receipt、AG-N6/AG-D3 专项证据及当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

收尾复核：全量 Node audit 1573/1573；本项并未生成或验收真实 Google receipt。

### 18.128 Claude 五 operation 静态模型目录接受前一致性复核

2026-10-09，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD/tree 未变且工作树干净；本项依据目标 Server 自身 `claude_code_wire_profile` 静态目录合同，不推导新 Anthropic wire。五项私有 receipt validator 原来只确认初次签名 Share 目录中所选模型恰好一次，不检查公开的来源、stale 与 capture 时间，也不在末次 Account/Provider/Share 复核后重读目录。五 operation 各构造第二次目录撤下模型、添加模型、stale、伪来源、capture 时间漂移五类反例，共二十五例旧流程均只请求首轮目录并错误输出 `contract_verified/live_pending`。现在初次目录须显式 `source=claude_code_wire_profile`、`stale=false`、capture 时间为正的安全整数；末次绑定后经同一签名 Share/Provider 栅栏重读，选定模型仍恰好一次，两次完整 `data` 摘要及元数据必须相同。二十五例先红后绿，正常五项 fixture 保持通过。

本项只收紧私有静态目录证据，不改变生产 Claude OAuth wire、CL-N7/CL-N8、canonical plan、receipt schema v1/revision 1 或模型目录内容；静态 capture 时间不套动态 TTL。完整目录摘要没有绑定进现有 receipt，只保证单次验收期间未漂移；分次读取不是跨控制面原子快照，fixture 不证明 Anthropic entitlement。Claude 专项 Node 220/220、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/provider/claude-oauth.md` 已同步双轮静态目录门槛。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；五份真实 receipt、续线程/beta 厂商接受性和当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

收尾复核：全量 Node audit 1599/1599；本项并未生成或验收真实 Anthropic receipt。

### 18.129 Codex Astra/WS 绑定目录与 Account manifest 接受前复核

2026-10-09，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交 HEAD/tree 未变、参考工作树干净；本项不推导新 OpenAI wire。目标 `gpt_6_astra` 和 `ws_prewarm` 私有 receipt validator 原先只在开始时从签名 Share/预期 Provider 栅栏读取公开 Codex 目录和绑定账号 `client_version` manifest，确认两者各包含一次选定模型；末次 Account/Provider/Share 复核后不再检查。两 operation 各构造第二次目录撤下或添加模型、manifest 撤下或添加模型、所选模型 `use_responses_lite` 能力变化五类反例，共十例旧流程均只读首轮目录/manifest 并错误输出 `contract_verified/live_pending`。现在末次绑定后重读两份证据，各自选定模型仍须恰好一次；完整目录 `data` 和 manifest `models` 分别按模型 id/slug 规范化顺序再比较摘要。十例先红后绿，manifest 仅行顺序变化的两项正例通过；三个 Image 2.5 operation 不套用此目录/WS 门槛。

本项只收紧私有模型证据，不改变生产 Codex HTTP/WS/Image wire、Share 路由、receipt schema v1/revision 1、Images disabled 或 WS prewarm 未启用决策。两份摘要没有写进既有 receipt，仅保证单次验收期间未漂移；分次读取不是跨 API 原子快照，fixture 不证明真实 ChatGPT 账号权益。Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/provider/codex-oauth.md` 已同步双轮门槛。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；五份真实 receipt 和当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

收尾复核：Codex 专项 Node 183/183、全量 Node audit 1613/1613、smoke Node 13/13；本项并未生成或验收真实 ChatGPT receipt。

### 18.130 Cursor API Key 完整目录接受前内容复核

2026-10-09，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 Cursor 参考对象经 `--check-sources` 复核，参考工作树中未提交的 Codex 内容继续排除。本项不推导新 Cursor protobuf/session wire。目标 API Key 私有 receipt validator 已在末次 Provider/Share 绑定后重读签名目录，核对所选模型唯一、`source=cursor_public_api`、非 stale 及首末两份时间戳新鲜，却不比较完整 `data`。两个 loopback 反例分别在第二次目录增加模型、改变所选模型 `capabilities.tools`；旧流程均错误输出 `contract_verified/live_pending`。现在首末目录各将完整 `data` 按模型 id 排序后计算摘要并比较，两例先红后绿；仅目录行顺序变化的正例也通过。OAuth 的静态 alias 目录不是账号 entitlement，不加同类门槛。

本项只收紧私有 API Key 目录证据，不改变生产 Cursor protobuf/session/目录 wire、OAuth/API Key 身份、Share 路由、目录 TTL 或 receipt schema v2/revision 2。摘要没有绑定进既有 receipt，只证明一次验收期间未漂移；分次读取不是跨控制面/上游原子快照，fixture 不证明厂商权益。Cursor 专项 Node 90/90、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/provider/cursor.md` 已同步完整目录门槛。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

收尾复核：全量 Node audit 1617/1617；本项并未生成或验收真实 Cursor receipt。

### 18.131 Grok 三 operation 目录 ID 完整性与顺序无关摘要

2026-10-09，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交参考对象经 `--check-sources` 复核；后者六项未提交 Codex 内容继续排除。GR-N4 同账号版本/header 接受性仍为 `live_pending_no_wire_change`，不推导新 xAI wire。目标 inference、media、remote_compaction 私有 receipt 已绑定初次完整目录摘要、末次绑定后再读目录，但旧 validator 仅要求所选模型 ID 唯一，允许其他条目缺 ID 或重复 ID；相同条目仅顺序变化时完整数组摘要不同，也会误拒绝。三项各构造其他 ID 重复、缺失两类反例，共六例旧流程错误输出 `contract_verified/live_pending`；三项各构造首末目录纯行顺序对调的正例，旧流程三例错误拒绝。现在要求每份目录全部 ID 非空、无首尾空白且唯一，并按 ID 排序再计算完整 `data` 摘要。六个负例与三个正例先红后绿；三项能力字段改变负例和既有模型增删、stale、伪来源、超前时间反例仍拒绝，合法来源切换仍可通过。

本项只调整私有证据规范化和失败关闭，不改变生产 Grok HTTP/WS/media/compaction wire、目录顺序/缓存、默认 CLI 版本、group header、receipt schema v1/revision 1 或 remote compaction disabled 决策；分次读取不是跨控制面/上游原子快照，fixture 不证明真实 xAI entitlement。Grok 专项 Node 155/155、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。`docs/provider/grok-oauth.md` 已同步摘要语义。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 接受性和当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

收尾复核：全量 Node audit 1631/1631；本项并未生成或验收真实 xAI receipt。

### 18.132 Kiro 八路 Claude/Codex 目录 ID 完整性与顺序无关摘要

2026-10-09，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交 HEAD/tree 未变且参考工作树干净；本项不推导新的 Kiro/Amazon Q EventStream wire。目标八个 auth-kind×region 私有 receipt 已绑定 Claude/Codex 两份完整目录摘要，并在末次绑定后重读；旧 validator 却只要求所选模型 ID 唯一，允许其他条目缺失、空白或重复 ID，且纯行重排会改变摘要。八路 × 两 App × 三种无效 ID 共四十八个反例在旧流程错误输出 `contract_verified/live_pending`；八路 × 两 App 的十六个纯重排正例在旧流程被错误拒绝。现在每份目录所有模型 ID 均须为非空、无首尾空白且唯一，并按 ID 排序后计算完整 `data` 摘要。上述反例先红后绿；十六个末次模型能力字段变化负例仍拒绝，原有模型增删、stale、伪来源、超前时间和合法来源切换回归保持通过。

本项只收紧私有证据规范化，不改变生产目录顺序/TTL、EventStream、auth kind/region 路由、KI-N5 计量权威、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策。分次读取不是跨控制面/上游原子快照，fixture 不证明真实厂商接受性。Kiro 专项 Node 492/492、全量 audit Node 1713/1713、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。八个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。八份真实 receipt、KI-N5 专项证据和当前未提交工作树的 committed-target observation 仍缺。最终整体 review 尚未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder，其推荐 TokenRouter 业务源码仍受当前会话缺失的 `project-doc` 技能门禁。

### 18.133 Qoder 三 rail 三 App 目录 ID 完整性与顺序无关摘要

2026-10-09，Qoder 单类别接续。推荐 TokenRouter 的 `AGENTS.md` 要求当前会话可调用 `project-doc`，而本会话未加载该技能；因此没有读取或运行其业务源码，也不据此外推新的 COSY wire。本项只核对目标 Server 自身私有 receipt 验收：旧 validator 将目录 ID 强制转字符串、去空白并过滤空值后用集合找三 App 公共模型，其他条目缺失、空白、重复或非字符串 ID 仍可通过；完整数组直接计算摘要又会误拒绝纯行重排。Global OAuth、Global PAT、CN OAuth × Claude/Codex/Gemini × 四种无效 ID 共三十六个反例在旧流程错误发布 `contract_verified/live_pending`；三 rail × 三 App 的九个纯重排正例在旧流程错误拒绝。现在每份初次和末次目录均要求所有 ID 为非空字符串、无首尾空白且唯一，并按 ID 确定性排序后计算完整 `data` 摘要。上述反例先红后绿；九个末次能力字段变化反例仍拒绝，原有模型增删、stale、伪来源、超前时钟回归不变。

本项只调整私有目录证据，不改变生产 COSY wire、site/rail/Account 路由、目录 TTL、receipt schema v2/revision 2 或既有 `live_pending` 决策。分次读取不是跨控制面/上游原子快照，fixture 不能证明真实厂商 entitlement。Qoder 专项 Node 281/281、全量 audit Node 1769/1769、smoke Node 13/13；自包含 Qoder reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。三条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。三份真实 receipt、登录/轮换、权威空目录、受控两段 401 和当前未提交工作树的 committed-target observation 仍缺；最终整体 review 尚未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.134 CodeBuddy 双站三 App 目录 ID 完整性与顺序无关摘要

2026-10-09，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 WorkBuddy `/v3/config` 实现仍作只读参考；参考工作树未跟踪的 `proxy.html`、`proxy.md` 排除。本项不推导新 CodeBuddy wire。目标 Intl/CN 私有 receipt 已分别绑定 Claude/Codex/Gemini 初次完整目录并在末次绑定后重读；旧 validator 只要求所选模型唯一，允许其他条目缺失、空白、重复或非字符串 ID，纯行重排又会改变摘要。双站 × 三 App × 四种无效 ID 共二十四个反例在旧流程错误输出 `contract_verified/live_pending`；双站 × 三 App 的六个纯重排正例在旧流程错误拒绝。现在每份目录的所有模型 ID 均须为非空字符串、无首尾空白且唯一，按 ID 确定性排序后计算完整 `data` 摘要。上述反例先红后绿；六个末次能力字段变化反例仍拒绝，原有模型增删、stale、伪来源和超前时间回归不变。

本项只收紧私有证据，不改变生产 CodeBuddy wire、站点身份、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled 决策。分次读取不是跨控制面/厂商原子快照，fixture 不证明真实账号接受性。CodeBuddy 专项 Node 186/186、全量 audit Node 1807/1807、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。Intl/CN 两个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。两份真实 receipt、CB-N4 CN 厂商目录/接受性和当前未提交工作树的 committed-target observation 仍缺；最终整体 review 尚未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.135 Antigravity/AGY 双 rail Claude/Gemini 目录 ID 完整性与顺序无关摘要

2026-10-09，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD/tree 未变且参考工作树干净；本项不推导新 Google wire。目标 `antigravity_oauth`、`agy_oauth` 私有 receipt 已绑定 Claude/Gemini 初次完整目录并在末次绑定后重读；旧 validator 只要求所选模型 ID 唯一，允许其他条目缺失、空白、重复或非字符串 ID，且纯行重排会改变摘要。双 rail × 两 App × 四种无效 ID 共十六个反例在旧流程错误输出 `contract_verified/live_pending`；四个纯重排正例在旧流程错误拒绝。现在每份目录所有模型 ID 均须为非空字符串、无首尾空白且唯一，按 ID 确定性排序后计算完整 `data` 摘要。上述反例先红后绿；四个末次能力字段变化负例仍拒绝，原有模型增删、stale、伪来源与超前时间回归不变。

本项只调整私有目录证据，不改变生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction、目录 TTL、receipt schema v2/revision 2 或 compaction disabled 决策。分次读取不是跨控制面/上游原子快照，fixture 不证明真实 Google 账号接受性。Antigravity 专项 Node 165/165、全量 audit Node 1833/1833、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。两份真实 receipt、AG-N6/AG-D3 专项证据和当前未提交工作树的 committed-target observation 仍缺；最终整体 review 尚未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.136 Claude 五 operation 静态目录 ID 完整性与顺序无关摘要

2026-10-09，Claude 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD/tree 未变且参考工作树干净；本项依据目标 Server 自身版本化 `claude_code_wire_profile` 静态目录，不推导新 Anthropic wire。目标五项私有 receipt validator 已在末次绑定后重读目录，却只要求所选模型 ID 唯一，允许其他条目缺失、空白、重复或非字符串 ID，且纯行重排会改变摘要。五项 × 四种无效 ID 共二十个反例在旧流程错误输出 `contract_verified/live_pending`；五个纯重排正例在旧流程错误拒绝。现在初次和末次目录所有 ID 均须为非空字符串、无首尾空白且唯一，按 ID 确定性排序后计算完整 `data` 摘要。上述反例先红后绿；五个末次能力字段变化负例仍拒绝，既有模型增删、stale、伪来源与 capture 时间漂移回归不变。

本项只收紧私有目录稳定性证据，不改变生产 Claude OAuth wire、静态目录内容、CL-N7/CL-N8、canonical plan 或 receipt schema v1/revision 1。摘要没有写入现有 receipt，分次读取不是跨 API 原子快照，fixture 不证明真实 Anthropic entitlement。Claude 专项 Node 252/252、全量 audit Node 1865/1865、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。五份真实 receipt、续线程/beta 厂商接受性和当前未提交工作树的 committed-target observation 仍缺；最终整体 review 尚未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.137 Codex Astra/WS 完整目录与 Account manifest 标识完整性

2026-10-09，Codex 单类别接续。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交 HEAD/tree 未变，参考工作树干净；本项不推导新 OpenAI wire。目标 `gpt_6_astra` 与 `ws_prewarm` 私有验收已重读公开 Codex 目录和绑定账号 manifest，并比较完整列表摘要，但只确认所选模型唯一，其他条目缺失、空白、重复或非字符串的 id/slug 均可能通过。两 operation × 目录/manifest × 四种无效标识共十六个反例在旧流程错误输出 `contract_verified/live_pending`。现在初次和末次完整目录的所有 id、manifest 的所有 slug 均须为非空字符串、无首尾空白且各自唯一，按确定性字符串顺序比较完整摘要。十六个反例先红后绿；纯目录/manifest 行重排保持通过，末次目录能力字段变化及原有模型增删、manifest `use_responses_lite` 变化仍拒绝。三个 Image 2.5 operation 不套此 Astra/WS 门槛。

本项只收紧私有模型证据，不改变生产 Codex HTTP/WS/Image wire、Share 路由、receipt schema v1/revision 1、Images disabled 或 WS prewarm 未启用决策。摘要未写入现有 receipt，分次读取不是跨 API 原子快照，fixture 不证明真实 ChatGPT 权益。Codex 专项 Node 205/205、全量 audit Node 1887/1887、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。五个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。五份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺；最终整体 review 尚未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.138 Cursor API Key 完整动态目录 ID 门槛

2026-10-09，Cursor 单类别接续。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 Cursor 参考对象继续作只读证据，参考工作树未提交的 Codex 内容排除；本项不推导新 Cursor protobuf/session wire。目标 API Key 私有 receipt 已在末次绑定后重读动态目录、按 ID 比较完整 `data` 摘要，但只要求所选 `*-fast` 模型唯一，其他条目缺失、空白、重复或非字符串 ID 仍可通过，坏 ID 排序时还被转成空串。四个 loopback 反例在旧流程错误输出 `contract_verified/live_pending`。现在初次和末次 API Key 完整目录所有 ID 均须为非空字符串、无首尾空白且唯一，再按确定性 ID 顺序计算摘要；四例先红后绿。纯行重排仍通过，原有模型增加、能力变化、stale、伪来源和时效反例保持拒绝。OAuth 静态 alias 目录不是账号 entitlement，不套此动态目录门槛。

本项只收紧私有 API Key 目录证据，不改变生产 Cursor protobuf/session/目录 wire、OAuth/API Key 身份、Share 路由、目录 TTL 或 receipt schema v2/revision 2。摘要未写入现有 receipt，分次读取不是跨控制面/上游原子快照，fixture 不证明真实 Cursor 权益。Cursor 专项 Node 95/95、全量 audit Node 1892/1892、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。OAuth/API Key 两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。两份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺；最终整体 review 尚未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.139 Grok 三 operation 目录摘要精确 ID 全序

2026-10-09，Grok 单类别接续。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 的提交态与前轮相同；`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 也无新提交，六处未提交 Codex 改动继续排除。参考对象 `--check-sources` 复核通过，本项只修目标私有 receipt 目录比较语义，不推断新的 xAI wire，也不改变 GR-N4 的 `live_pending_no_wire_change` 决策。现有目录 ID 已要求非空、无首尾空白且唯一，但摘要排序使用 `localeCompare`；字节不同的 `café` 与 `cafe\u0301` 会比较为相等，稳定排序因而保留原行序。三个 operation 各以只交换这两个条目的首末目录构造回环正例，旧流程三例全部错误拒绝。现按精确 JS 字符串全序比较 ID，三个正例转绿，其他能力字段仍由完整 `data` 摘要绑定；对象字段的既有 canonical 及 receipt schema v1/revision 1 不变。

Grok 专项 Node 159/159、全量 audit Node 1896/1896、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 接受性及当前未提交工作树的 committed-target observation 仍缺。本项仅改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；离线 fixture 不证明真实 xAI 权益，未做 UI 自动化或部署测试。下一单类别为 Kiro，最终整体 review 仍未完成。

### 18.140 Kiro 模型目录缓存的凭据、账号实例与机器身份隔离

2026-10-09，Kiro 单类别接续。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` / tree `5d22f040f0e4b0aa775e61547540b3c040293cd8` 的提交态未变、参考工作树干净；本项依据目标自身动态目录缓存和 Account 生命周期修复，不从参考推导新 EventStream wire。原 `ModelCacheKey` 已含 App、Provider revision/runtime、Account ID、auth/token generation、profile ARN 和运行区域，却不含实际 access token、进程内账号 incarnation，也未覆盖 ListAvailableModels 所发的机器身份。三个 loopback 反例分别保持同账号 ID/代际/profile 而替换 token、以同 ID/同 token/同代际重建账号实例、保持 token/profile 而更换机器 ID；旧流程三例均直接返回旧目录，未向新身份发网。

现在缓存键加入 AccountStore 的非持久 incarnation，以及 access token、认证方式、token 类型和机器 ID 的域分离 SHA-256 请求身份摘要；原文不进入新缓存键。`state.rs` 从同一次 AccountStore 读取得到账号和 incarnation，强制 refresh 后仍要求相同实例，返回目录前再核对实例、代际、请求身份、有效 profile/区域与 Provider 绑定。三例先红后绿；测试作用域以外的生产 Kiro/Amazon Q wire、模型目录 TTL、缓存来源、KI-N5 计量权威、receipt schema v1/revision 1 及 Compact/shared cache disabled 决策不变。此处的账号实例只在进程内有意义，跨控制面读取并非原子事务。

完整 Rust library 3294 passed / 1 ignored，其他目标 124 + 1 + 1 passed；`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、Kiro 私有 receipt Node 492/492、全量 audit Node 1896/1896、smoke Node 13/13、Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 与文档索引通过。`src/state.rs` 的 Phase 0 writer 源码摘要由生成脚本同步。本轮八个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，八份真实 receipt、KI-N5 专项证据、Compact/shared cache 的独立验收及当前工作树 committed-target observation 仍缺；不把回环 fixture 当作真实厂商接受性。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Qoder。

### 18.141 Qoder 生产 session/目录缓存的请求身份与账号实例隔离

2026-10-09，Qoder 单类别接续。推荐 TokenRouter 的 `AGENTS.md` 要求当前会话可调用 `project-doc`，但当前技能列表没有该项；依仓库门禁未读取其业务源码，也未运行外仓 `--check-sources`，最新参考增量继续为 `unreviewed_skill_gate`。本项只依据目标 Server 自身缓存、Account 生命周期和请求身份实现，不推导新 COSY wire。原 `QoderRuntimeScope` 包含 App、Provider revision/runtime、Account ID、站点/rail 与 auth/token generation，却不含进程内账号实例、实际凭据、profile/机器 token 或有效期。同 ID 同代次重建或直接变更凭据/机器身份可能复用旧 session 与权威模型目录；`runtime_is_current` 也只复核两种 generation。

现在将 AccountStore 的非持久 incarnation 与域分离 SHA-256 请求身份摘要加入 session/catalog 作用域；摘要覆盖实际 access/refresh/PAT 凭据、profile、机器 token、有效期、重新登录状态及测试端点覆盖，明文不进入缓存键。强制刷新后、缓存返回前、网络目录写入前与 generation 前均复核当前 Provider/Account 绑定、实例、代次和请求身份。缓存隔离单测覆盖同代次不同凭据与同 ID 重建；生产回环覆盖同代次 token/机器 ID 改变后第二次请求重新拉目录并生成新请求，既有三 rail/三 Surface 回归仍通过。复核不构成跨存储原子事务，进程内 incarnation 不承诺跨重启持久稳定；本项不改变 COSY wire、目录 TTL、rail/site 选择或 receipt schema。

Qoder 关键词 79/79、完整 Rust library 3297 passed / 1 ignored，其他目标 124 + 1 + 1 passed；全量 audit 与 smoke Node 测试、Clippy `--all-targets -D warnings`、rustfmt、Qoder CLI oracle、自包含 reference-delta、Server Provider/Phase 0/coverage/UI matrix 与文档索引通过。三个 `RUN_REAL=0` 分别返回 `blocked_inputs/live_pending`，三份真实 receipt、登录/轮换、权威空目录、受控两段 401、最新 TokenRouter 合规补审与当前工作树 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.142 CodeBuddy 双站生产目录缓存的同代次请求身份隔离

2026-10-09，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD 中，自上次已审 `ead3202` 后没有新的 `internal/providers/workbuddy/` 专属提交；参考工作树两个未跟踪文档继续排除。本项依据目标 Server 自身 Account 生命周期与动态目录缓存复核，不从其他 Provider/共享改动推导新的 CodeBuddy wire。旧 `CodeBuddyRuntimeScope` 包含 App、Provider revision/runtime、Account ID、site/domain 与 auth/token generation，但同代次直接更换 access token 或 UID、同 ID 同代次重建账号仍可命中旧目录，且发起 generation 前只复核两种 generation。Intl/CN 两站的旧回环先在 access token 变化后各错误复用旧目录。

现在目录 scope 加入 AccountStore 非持久 incarnation 和域分离 SHA-256 请求身份摘要，覆盖 access/refresh 凭据、profile、有效期、重新登录状态与测试端点覆盖；缓存键不保留明文。目录 fetch 前后、缓存返回时和 generation 前均复核 Provider/Account 绑定、实例、代次及请求身份；同代次凭据/UID 变化会重新取目录，旧 stale fallback 也只能在完全相同的新作用域使用。Intl/CN × 凭据/UID 四个生产回环转绿；缓存单测验证同代次凭据变化与同 ID 账号重建不能命中旧目录。复核不是跨存储原子快照，incarnation 仅在进程内有效；本项不改上游 URL/header/payload、目录 TTL、Share 选择、Intl/CN 站点、receipt schema v2/revision 2 或 CB-N4 disabled 决策。

完整 Rust library 3300 passed / 1 ignored，其他目标 124 + 1 + 1 passed；全量 Node audit 1896/1896、smoke 13/13，Clippy `--all-targets -D warnings`、rustfmt、CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引与差分空白检查通过。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有发起真实请求。两站真实 receipt、CB-N4 国内厂商目录/接受性以及当前未提交工作树的 committed-target observation 仍缺；离线回环不能提升 `live_pending`。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.143 Antigravity/AGY 同代次 Bearer 目录回退隔离

2026-10-09，按循环回到 Antigravity。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD、只读参考边界与前次一致；本项只修目标自身 AG-N12 缓存身份缺口，不从参考推导新 Google wire。原 `fetchAvailableModels` 缓存键含 Provider type、Account ID/incarnation、auth/token generation、project 与 URL，却不含实际请求 Bearer。即使代际不变，先用 A 成功取得目录，再将账号 token 改为 B 并令上游返回 503，旧流程仍以同键返回 A 的 stale entitlement；反例先红。能力证据写入与 API 返回前也只复核代际、实例和项目，不能识别同代次 token 变化。

现用域分离 SHA-256 摘要绑定实际 Bearer 与重新登录状态，明文不进入缓存键。A/B 的 stale fallback 不能互通；同一精确键的发布序号仍防较早请求覆盖较新请求，不同 Bearer 的并行键互不删除，避免旧 A 晚返回清掉 B 的可用目录。为防同代次异常频繁换凭据扩大 retained state，目录总条目限为 128，按获取时间/请求序号淘汰最旧项；淘汰只会使 stale 回退不可用，不会跨凭据补齐。API 返回及 Account 能力证据写入前再复核当前请求身份；同代次 token 改变时旧 stale 与 fresh 证据均拒绝。回环 stale 反例、同代次双凭据倒序发布、容量上限和既有成功空目录/账号重建/能力证据 fixture 转绿。此处同代次直接改变 token 是防御性边界；正常 refresh 仍应提升 token generation。成功空目录、24 小时 stale 上限、AG-D3、AG-N6/requestType、compaction disabled 和两条 rail 的真实验收决策均不改变。

完整 Rust library 3303 passed / 1 ignored，其他目标 124 + 1 + 1 passed；Antigravity/Phase 0 专项 Node 171/171，全量 audit Node 1896/1896、smoke Node 13/13；Clippy `--all-targets -D warnings`、rustfmt、Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引和差分空白检查通过。两条 `RUN_REAL=0` 分别为 `blocked_inputs/live_pending`，没有发起真实请求。两份真实 receipt、AG-N6/AG-D3 专项接受性及当前未提交工作树的 committed-target observation 仍缺，不能将回环 fixture 解释为 Google live。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.144 Claude 续线程别名的账号实例与凭据隔离

2026-10-09，Claude 单类别接续。只读核对推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `claude_executor_tool_state.go`；其 message ID 别名存储没有本 Server 多用户 Share/Account 的身份边界，因此本项按目标自身 CL-N7 缓存和 Account 生命周期设计，不推导新的 Anthropic wire。旧作用域已有 Share、用户、显式 session、Provider revision/runtime、Account ID 和身份代次，却没有进程内账号实例、token refresh 代次或实际出站凭据。相同 ID/代次重建账号，或直接改 access token 而代次不变，可能错误复用旧工具别名。

现以 Account incarnation、token refresh generation 和域分离 SHA-256 摘要隔离别名；摘要包含最终 Authorization Bearer 与当前账号 refresh token、profile、有效期、重新登录状态，缓存键不保留明文。若当前账号 access token 与最终出站 Authorization 不一致，当前请求不读写续线程别名，避免并发变化导致旧 wire 被记在新身份下。同代次改 token 的回环在发网前拒绝无声明续接，恢复原 token 后仍能命中原状态；单元 fixture 覆盖账号重建和摘要变化。缓存仍只有进程内 10 分钟/1024 条/8 MiB 上限，不提供跨重启恢复，也不改变 Claude OAuth wire、Share 路由、CL-N8、模型目录或 receipt schema。

完整 Rust 3303 passed / 1 ignored，其他目标 124 + 1 + 1 passed；Node audit/smoke 1909/1909，Clippy `--all-targets -D warnings`、rustfmt、Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、文档索引与差分空白检查均通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有发起真实请求。五份真实 receipt、thread/beta 厂商接受性和当前未提交工作树的 committed-target observation 仍缺；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.145 Codex previous-response 工具上下文的账号实例与凭据隔离

2026-10-09，Codex 单类别接续。只读核对推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 Codex 续传实现和 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 按下游 API Key ID 分 owner 的本地 response cache；后者的匿名空间不适合移入本 Server 的签名 Share。目标旧 `PreviousResponseCacheScope` 只有 Share、principal、runtime fingerprint 与 workspace；同一账号同代次直接换 token 后，新请求仍会命中旧 `response_id` 的工具上下文。精确作用域回归先在旧实现复现相等的缓存键。

现新增 Provider ID/revision、Account ID/incarnation、auth/token generation 与域分离 SHA-256 凭据摘要，覆盖 access/refresh/ID token、token type、extra headers、verified subject、有效期与 relogin 状态；缓存键不保留明文。HTTP 和 WebSocket 共用作用域，缓存实体与 tombstone 都按新键隔离。换 token 后旧工具上下文未命中，恢复原 token 后可命中原键；缓存层 fixture 验证同 ID 账号实例变化。HTTP 在最终 OAuth Authorization 形成后再次核对：已注入历史上下文时若身份漂移，发网前 409；无需旧上下文的请求改在最终凭据作用域写入新成功状态。WebSocket 每个新 `response.create` 都比对当前账号与连接 Authorization，凭据变化要求重连。绑定 Share 的账号禁止直接删除，测试不绕过这项订阅身份保护。此改动不改变 OpenAI/Codex wire、模型目录、WS/HTTP fallback、普通 continuation 与缺历史上下文的 409 判定、TTL/容量、receipt schema 或真实验收状态。

Codex 关键词 Rust 409/409、完整 library 3305 passed / 1 ignored、其他目标 124 + 1 + 1 passed；Node audit/smoke 1909/1909，Clippy `--all-targets -D warnings`、rustfmt、Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引与差分空白检查通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有发起真实请求。五份真实 receipt、当前未提交工作树的 committed-target observation 仍缺；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Cursor。

### 18.146 Cursor parked h2 与 Responses 续传的精确凭据隔离

2026-10-09，Cursor 单类别接续。只读核对推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 `open-sse/services/cursorSessionManager.ts`：其 parked h2 stream 按 conversation ID 保留并由同一流接收工具结果，适合单层代理的续传，但不能直接用作本 Server 的多租户 Share/账号身份边界；参考工作树未提交 Codex 内容继续排除，不推导新 Cursor protobuf 或厂商 wire。目标 OAuth session 原来只含账号 ID、auth/token generation，同代次直接换 access token、机器 ID 或同 ID 重建可找到旧 h2 stream；completed Responses scope 甚至没有 token generation。OAuth 旧作用域在换 token 的定向回归中先证明相等。API Key 在已验证账号 ID 不变时，同代次更换 key 也可能复用旧 session。

现在 OAuth session 与 completed Responses 共用 AccountStore 同一快照中的进程内 incarnation 和域分离 SHA-256 请求身份摘要，绑定规范化 access token、Cursor 机器/配置身份、refresh/ID token、token type、extra headers、有效期和重新登录状态；API Key session 即使验证账号 ID 相同也绑定实际 key 摘要。作用域不存明文。取用 parked stream、解析凭据、开启 h2 后及会话就绪时复核当前作用域；漂移时关闭已取得的 stream 并返回冲突。正常同绑定 401 refresh 仍可重建 session key，成功 Responses 状态改按刷新后的最终作用域写入。conversation affinity 故意可跨 refresh 保持稳定，但实际 session/response 查找必须有精确作用域。单测覆盖 OAuth 同代次 token/机器变化、同 ID 账号重建和已验证 API Key 同代次轮换；既有 401 恢复回归保持通过。这些分次读写不构成跨存储原子事务，incarnation 只在进程内有效；生产 wire、目录、receipt schema 均不改变。

完整 Rust library 串行 3307 passed / 1 ignored，其他目标 124 + 1 + 1 passed；首次并行全量仅账号额度等待的既有时序测试预期 400、实际 502，单独复跑与串行全量均通过。Cursor 私有 receipt Node 95/95、全量 Node audit 1896/1896、smoke 13/13，Clippy `--all-targets -D warnings`、reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引、rustfmt 和差分空白检查通过。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，两份真实 receipt 与当前未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Grok。

### 18.147 Grok 模型目录与 reasoning 能力的同代次 Bearer 隔离

2026-10-09，Grok 单类别接续。只读核对推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 的已提交 CLI 模型请求与 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的 observed-model 请求：两者都用当前账号 Bearer 请求 `/v1/models`，但不直接提供本 Server 多租户 Share 的目录授权模型；后者六项未提交 Codex 文件继续排除。GR-N4 同账号版本/header 接受性仍为 `live_pending_no_wire_change`，本项不推断新 xAI wire。目标旧 `GrokModelCatalogScope` 已含 Account incarnation 与两种 generation，却不含实际 Bearer。同 ID、同代次直接将 access token 从 A 改为 B 后，旧生产回环仍向 B 暴露 A 的 fresh reasoning capability；反例先红。

现为目录 scope 增加规范化实际 Bearer 与 relogin 状态的域分离 SHA-256 摘要，不在缓存键保留明文。Provider/Account 绑定和请求身份在发网前、401 强刷前、结果接受前复核；A 的迟到 401 不得强刷当前 B。缓存发布仍按账号实例、Provider revision、auth/token generation 拒绝旧代际，但同代次不同摘要各留精确条目，避免 A 的迟到请求驱逐 B；全局最多保留 128 条，按抓取时间与请求序号淘汰。`ServerState` 的 fresh capability lookup 从同一账号快照派生摘要，B 无法读取 A 的 reasoning 菜单。测试覆盖同代次换 token 后重新发目录请求、旧能力失效、双 Bearer 发布和容量边界；正常单账号 401 恢复仍保持同绑定。分次复核不是跨存储原子事务，未改变模型 URL/header、TTL、ETag/304、stale 展示、HTTP/WS wire 或真实 receipt schema。

完整 Rust library 串行 3310 passed / 1 ignored，其他目标 124 + 1 + 1 passed；Grok 私有 receipt Node 159/159、全量 audit Node 1896/1896、smoke Node 13/13。Clippy `--all-targets -D warnings`、rustfmt、Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引和差分空白检查通过；Phase 0 的 writer/compatibility 生成摘要已随源码更新。inference、media、remote_compaction 的 `RUN_REAL=0` 分别返回 `blocked_inputs/live_pending`，三份真实 receipt 和 GR-N4 同账号版本/header 厂商证据仍缺。当前工作树未提交，不能生成本轮 committed-target observation；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Kiro。

### 18.148 Kiro SQLite 本地 prompt-cache 估算的同代次凭据隔离

2026-10-09，Kiro 单类别接续。只读核对推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的提交态及既有 KI-N5 shadow 门槛；本项只修目标 Server 自身的本地估算身份边界，不从参考推导新 EventStream wire。生产模型目录缓存先前已用实际请求身份隔离，但 SQLite prompt-cache namespace 只有 Account ID、auth/token generation、Provider、Share、用户、session 和 region。固定 session、账号与代际后只换 access token，旧 namespace 完全相同的红灯回归已复现。

现将独立域的 SHA-256 请求身份摘要加入 prompt-cache namespace，覆盖规范化出站 Bearer、refresh token、实际机器 ID、profile ARN、区域、endpoint、认证方式、token type 和 relogin 状态；原文不写入 namespace。直接调用 fallback 与绑定 Provider 两条入口均使用同一函数；401 刷新后的转发循环重新准备请求，因此重建 namespace。摘要由持久化 Account 字段稳定导出，**不加入非持久 Account incarnation**，保留 KI-03 相同身份在进程重启后 TTL 内命中 SQLite 的合同。换 Bearer/机器身份不复用旧估算，同凭据恢复原键；新增 SQLite 重启命中与换凭据 miss 回归。相同 ID、代际及全部有效身份字段的删除重建仍不能由此区分，若要完全解决须另行引入持久账号实例标识；该剩余边界只影响本地估算，不改变上游 wire/账单。

KI-N5 context usage 仍只作 shadow，缺八个 auth-kind×region 真实 receipt 前不切换生产计量权威；Compact/shared cache 保持 disabled。此轮不做 UI 自动化或部署测试，不把离线 fixture 当作厂商接受性。完成 Kiro 离线验证后下一单类别为 Qoder，其推荐 TokenRouter 最新业务源码仍需重新核对 `project-doc` 技能门禁；全部项目的最终整体 review 尚未完成。

Kiro 定向 Rust 195/195，完整 Rust library 3312 passed / 1 ignored，集成目标 124 + 1 + 1 passed；默认测试线程栈下既有 CodeBuddy provider-test 单独复现栈溢出，设置 `RUST_MIN_STACK=16777216` 后单测及完整串行套件均通过，未将此跨类别现象归因于 Kiro。`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、Kiro receipt Node 492/492、全量 audit Node 1896/1896、smoke Node 13/13、Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引及差分空白检查通过。八个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，八份真实 receipt、KI-N5 专项证据和当前未提交工作树的 committed-target observation 仍缺；本轮不生成伪提交态记录。下一单类别为 Qoder。

### 18.149 Qoder 公开目录、alias 路由和精确模型键一致性

2026-10-09，Qoder 单类别接续。推荐 TokenRouter 的 `AGENTS.md` 仍要求本会话不可用的 `project-doc` 技能；只读取门禁说明，没有读取其业务源码或运行访问外仓的 `--check-sources`，最新源码增量继续为 `unreviewed_skill_gate`。本项只修目标 Server 自身的目录/请求语义，不推导新的 COSY wire。旧实现对 live catalog 的每个启用原始 key 都先投影公开模型，再按 ID 排序去重；但请求解析会优先将公开 alias 改写到固定 route。若上游同时启用 `claude-opus-4-6` 原始 key 而未启用 `ultimate`，目录会发布一个实际请求必然 403 的 ID；两者都启用时还可能用原始 key 的 capability 冒充 `ultimate`。未知厂商 key 一律被转小写，带首尾空白的目录 key 被裁剪，又让公开 ID、请求 route 和上游原始 `model_config` 不一致。三组定向测试在旧实现先红。

现在目录 parser 要求上游 key 非空、无首尾空白并原样存储；请求解析只对已审 Global/CN alias 做 ASCII 大小写不敏感匹配，未知 key 保持精确大小写。公开模型必须满足“以该 ID 请求时仍解析到同一个启用的原始 key”，否则跳过被 alias 遮蔽的原始条目；只有真实启用的 alias route 才发布 alias ID 与其 capability。两站 alias 列表逐项反例、同名原始 key 与真实 route 双启用、未知大小写和空白 key 回归转绿。成功空目录、独立账号/site/rail 绑定、模型目录 TTL、请求体 wire、receipt schema v2/revision 2 均不变；这不证明 TokenRouter 最新实现或真实 Qoder entitlement。

Global OAuth、Global PAT、CN OAuth 的登录/轮换、权威空目录、受控两段 401 及三个真实 rail receipt 仍分别待证，当前工作树也没有可追加的 committed-target observation；不要将离线 fixture 升为 live。Qoder 离线验证收口后下一单类别为 CodeBuddy。最终整体 review 尚未完成，不做 UI 自动化或部署测试。

Qoder 定向 Rust 81/81，完整 Rust library 3314 passed / 1 ignored，其他目标 124 + 1 + 1 passed（以 `RUST_MIN_STACK=16777216` 串行运行，绕开前轮确认的既有 CodeBuddy 默认栈溢出）；`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、Qoder receipt Node 281/281、全量 audit Node 1896/1896、smoke Node 13/13、自包含 Qoder reference-delta、CLI oracle、Server Provider/Phase 0/coverage/UI matrix、文档索引与差分空白检查通过。三条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；未读取受门禁的 TokenRouter 业务文件，未做 UI 自动化或部署测试，未提交本轮工作树。下一单类别为 CodeBuddy。

### 18.150 CodeBuddy `/v3/config` 业务码与原始模型 ID 的精确门槛

2026-10-09，CodeBuddy 单类别接续。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 WorkBuddy `client.go::Models` 从 `/v3/config` 读取权威目录、失败不以静态目录补齐；`catalog.go::catalogModel` 原样使用上游 `model.ID`，CLI allowlist 也按精确 ID 匹配。参考工作树未跟踪的 `proxy.html`、`proxy.md` 继续排除。本项不推导新的聊天 wire，而是按目标 Server reviewed allowlist 的更严格身份边界复核目录。旧 parser 把每个上游 ID 裁剪并转小写；`FAST-MODEL` 因而可被错误提升为 Intl 已审核 `fast-model`，带首尾空白的 ID 也被悄悄改写。若顶层业务 `code` 存在但不能解析，旧逻辑会将其视作缺字段，继续接受看似有效的 `data.models`。两组定向回归在旧实现先红。

现在要求每个目录 ID 非空且无首尾空白，按上游原文做精确重复检查和站点 reviewed allowlist 交集；大小写不同的启用项只作为未审核漂移计数，不改写成审核模型。顶层业务 `code` 若存在则必须可解析且等于 0，非数字字符串、`null`、布尔或对象失败关闭；缺 `code` 的既有兼容响应和可解析字符串 `"0"` 保持可用。成功空目录、同站同账号的 stale 策略、目录 TTL、Provider/Share/Account 绑定、非流/流 wire 与 receipt schema v2/revision 2 不变；CB-N4 `deepseek-v4.1-flash` 仍 disabled。CodeBuddy parser 定向回归转绿，真实 Intl/CN 厂商目录及两站 receipt 仍需独立验收。当前未提交工作树不能伪造 committed-target observation，最终整体 review 尚未完成；不做 UI 自动化或部署测试。离线验证收口后按循环回到 Antigravity。

CodeBuddy 定向 Rust 77/77（含 parser 13/13），完整 Rust library 3316 passed / 1 ignored，其他目标 124 + 1 + 1 passed；`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、CodeBuddy receipt Node 186/186、全量 audit Node 1896/1896、smoke Node 13/13、CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引与差分空白检查通过。Rust 完整与 CodeBuddy 定向套件以 `RUST_MIN_STACK=16777216` 运行；默认线程栈下既有 CodeBuddy Provider test 仍会栈溢出，单纯将外层测试 future 装箱无效且该尝试已撤回，不能声称问题解决。Intl/CN 的 `RUN_REAL=0` 分别为 `blocked_inputs/live_pending`，无真实请求；本轮未做 UI 自动化或部署测试。下一单类别按循环回到 Antigravity。

### 18.151 Antigravity 模型目录迟到成功响应的发布一致性

2026-10-09，按循环只处理 Antigravity。只读核对推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 和 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的冻结提交态；Manager 的 `fetchAvailableModels` 以原始模型 key 保留目录，但本项不推断新的 Google wire。目标自身 AG-N12 的目录缓存已按账号实例、身份代际与请求 Bearer 隔离，`publish_catalog` 也会拒绝旧请求覆盖新目录；然而调用方忽略发布结果，旧请求若晚于同一精确键的新请求成功返回，仍把旧目录当 `stale=false` 返回。倒序回环在旧实现先得到 `gemini-old`，而已发布缓存是 `gemini-new`。

现在成功响应发布遭拒时，在同一缓存锁下查找精确键：若较新的同身份目录存在，返回其完整 descriptors、revision 与抓取时间，仍作为该次已有权威目录的非 stale 展示；若新账号代际已经取代旧键，返回不可重试的 409，不把已失效身份的晚到成功响应当作 entitlement。控制层保留 409，不再统一改为 502。两种倒序回归和既有晚到旧 Bearer/成功空目录/缓存容量 fixture 覆盖此边界。成功空目录、同代次不同 Bearer 并行键、24 小时 stale fallback、AG-N6/requestType、AG-D3、compaction disabled、双 rail receipt schema 和真实验收门槛不变；缓存锁只保证本进程发布一致性，不构成跨存储原子快照。

两条真实 `antigravity_oauth` / `agy_oauth` receipt、AG-N6 与 AG-D3/compaction 专项厂商证据以及当前未提交工作树的 committed-target observation 仍缺，保持 `blocked_inputs/live_pending`；不得把回环 fixture 升级为真实 Google 接受性。下一单类别为 Claude，最终整体 review 尚未完成；本轮不做 UI 自动化或部署测试。

验证收口：最终代码的完整 Rust library 3319 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216` 串行运行，继续绕开既有 CodeBuddy 默认线程栈溢出）；`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、Antigravity 私有 Node 165/165、全量 audit Node 1896/1896、本地 smoke Node 13/13 与服务回环均通过。Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引、差分空白检查也通过。因控制层源码变化，按 Phase 0 生成器更新 Provider writer 的当前源码摘要；全量 Node 首次 4 项失败均由旧摘要触发，重新生成后全绿。服务 smoke 首次与 Rust 编译锁争用，预构建后重跑通过。离线 readiness 按 `RUN_TESTS=0 RUN_REAL=0` 返回预期 `blocked_inputs`：明确跳过测试并缺真实凭据/部署验收，不把它计为通过。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有发起真实请求。

### 18.152 Claude 续线程 message ID 重用时的别名冲突隔离

2026-10-09，按循环只处理 Claude。只读复核推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `claude_executor_tool_state.go`：参考以 message ID 保存、后写覆盖工具别名，未提供本 Server 的 Share/用户/Account 身份边界。目标已有更精确的作用域、容量和 TTL，但同一精确作用域的两个成功响应若异常复用同一上游 message ID、却携带不同别名映射，旧缓存仍后写覆盖。后续 continuation 会任选最后一份映射，可能错误改写历史工具名。缓存单测先复现覆盖反例。

现在同一精确 key 的异映射重复发布会将该 key 标记为冲突，保留原 TTL/容量记账；之后的发布不能解除冲突。lookup 区分缺失与冲突：缺必需历史状态仍为原有 404；已知冲突则即使客户端重新携带工具声明，也在发网前返回 409，要求通过 thread create 重放完整会话。同映射重复发布仍可正常刷新；其他 Share、用户、session、Provider/runtime、Account 实例和凭据作用域不受影响。这是目标 Server 的本地状态安全边界，不推断新的 Anthropic thread wire；TTL 到期、进程重启或容量淘汰后不承诺识别曾经冲突的 ID。五项真实 operation receipt 与续线程/beta 厂商接受性仍独立 `live_pending`，不能用回环 fixture 提升。

本轮不做 UI 自动化或部署测试；当前未提交工作树不能补写 committed-target observation。离线验证收口后下一类别为 Codex，最终整体 review 尚未完成。

验证收口：缓存层异映射 message ID 回归先红后绿，真实转发回环证明带新工具声明的续接在冲突时仍不发第三次上游请求。最终完整 Rust library 3321 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216` 串行运行，沿用既有 CodeBuddy 默认栈溢出的规避条件）；`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、Claude 私有 Node 252/252、全量 Node audit 1896/1896、Node smoke 13/13 与本地服务 smoke 均通过。Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、文档索引及差分空白检查通过；因 `state.rs` 源码变化，Phase 0 Provider writer 摘要由生成器刷新。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发起真实请求。

### 18.153 Codex previous-response 不透明 ID 的精确身份边界

2026-10-09，按循环只处理 Codex。只读核对推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的冻结提交态；参考项目中的 ID 裁剪不能作为本 Server 签名 Share/用户/账号工具上下文的授权规则。目标原有 `previous_response_id_from_value` 和 `PreviousResponseCache::cache_key` 均对响应 ID 调用 `trim()`。同一精确 scope 内预存 `resp-a` 的历史 tool call 后，以 `" resp-a"` 或 `"resp-a "` 请求，旧代码会错误复用 `resp-a` 的工具历史并注入当前请求。缓存命中与请求提取两条反例先红。

现在请求提取保留原始字符串；缓存键仅接受非空、至多 512 字节、无首尾空白的精确 ID，写入完成响应时也不再裁剪上游 ID。HTTP 与 WebSocket `response.create` 共用这两个边界；带未配对 tool output 的无效/未命中 ID 因找不到历史按现有 `response_context_unavailable` 409 在发网前失败关闭，普通无依赖 continuation 仍沿用既有清洗/转发行为。合法 ID、同账号精确作用域、TTL/容量、tombstone、Share 策略、成功 terminal 写入、实际 Codex wire 与五份 receipt schema 均不变。这是目标本地身份一致性修复，不从参考推断新的 OpenAI 协议，也不能把回环当真实厂商接受性。

五个真实 operation receipt、Astra/Image 2.5/WS prewarm 厂商接受性及当前未提交工作树的 committed-target observation 仍缺，继续 `live_pending`；最终整体 review 尚未完成。本轮不做 UI 自动化或部署测试，离线验证收口后下一类别为 Cursor。

验证收口：缓存键与请求提取两条红灯先复现错误裁剪，修复后连同无历史工具输出的精确注入回归均通过。最终完整 Rust library 3324 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216` 串行运行，沿用既有 CodeBuddy 默认栈溢出的规避条件）；`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、Codex 私有 Node 205/205、全量 Node audit 1896/1896、Node smoke 13/13、本地服务 smoke 均通过。Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引及差分空白检查通过。`RUN_TESTS=0 RUN_REAL=0` 离线 readiness 因明确跳过测试、缺真实凭据和部署验收返回预期 `blocked_inputs`，不能计为发布通过；五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，无真实请求。

### 18.154 Cursor 续传 ID 的精确身份边界

2026-10-09，按循环只处理 Cursor。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 Cursor session/executor 路径仍只作 parked-stream 机制参考；22 项未提交 Codex 工作树内容继续排除，CUR-N2 保持 `reviewed_no_wire_delta`。目标 Server 自身的已完成 Responses 缓存键、parked response/tool 索引与消费 claim、工具结果配对和 Responses 历史去重原先会裁剪不透明 ID。`" resp-a"` 因而可能命中 `"resp-a"` 的已有状态，`" call-a "` 可能被视为另一工具调用的结果。缓存、session、请求构造三组反例在旧实现先红；历史去重新增同类回归。

现在缓存与 parked 索引只接受非空、无首尾空白的原始 ID；工具结果校验按原始 `call_id` 去重并拒绝空白边界，cold-resume 只按精确结果 ID 判定，Responses 历史合并对调用和结果 ID 均拒绝首尾空白，错误不回显客户端 ID。复核发现 Gemini 显式 function-response ID 在提取阶段也被裁剪，现保留原值交由统一校验拒绝；省略 ID 时既有 name fallback 不变，新增 Gemini 反例。这样带空白的续接不能借用另一条已完成响应、parked stream 或已消费 claim；合法 ID、原有精确 Share/用户/Provider/凭据作用域、TTL/容量、OAuth/API Key 双 rail、protobuf wire 与 receipt schema 均不变。这是本地状态身份安全修复，不推导新厂商 wire，也不把离线 fixture 当成真实 Cursor entitlement。

两份 OAuth/API Key 真实 receipt 和当前未提交工作树的 committed-target observation 仍缺，保持独立 `live_pending`；最终整体 review 尚未完成。不做 UI 自动化或部署测试。下一单类别为 Grok。

验证收口：缓存、session、结果匹配三处反例在旧实现先红；修复后连同 Responses 历史合并和 Gemini 显式 ID 回归均通过。最终代码完整 Rust library 3329 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216` 串行运行，沿用既有 CodeBuddy 默认栈溢出的规避条件）；`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、Cursor 私有 Node 95/95、全量 Node audit/smoke 1909/1909、本地服务 smoke 均通过。Cursor reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引及差分空白检查通过。OAuth/API Key 的 `RUN_REAL=0` 分别为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因显式跳过本地测试、缺真实凭据和未做部署验收返回预期 `blocked_inputs`，不能计为发布通过。

### 18.155 Grok reasoning replay 的同代次账号请求身份隔离

2026-10-09，按循环只处理 Grok。只读核对推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 的已提交 CLI adapter 使用当前账号 Bearer；`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 文件继续排除。参考不定义本 Server 多租户 reasoning proof 的授权作用域，GR-N4 同账号版本/header 门槛仍不借此改 wire。目标目录缓存已经按实际 Bearer 隔离，但本地 reasoning replay scope 只有 Account ID 和 auth/token generation。固定 Share、用户、session、模型、turn、Provider 与同一账号代际，仅把 access token 从 A 改成 B，旧 HTTP 回环错误地为 B 注入 A 的 opaque reasoning proof；定向反例先红。

现在状态层在单个账号读锁下取得进程内 Account incarnation、token generation 与已有的规范化 Bearer/relogin 域分离 SHA-256 指纹；replay scope v2 将实例和指纹纳入哈希，HTTP/WS 共用。成功终态提交前再精确复核，旧身份迟到响应不能写入当前身份的 proof；同身份正常 replay、既有 CAS、30 分钟 TTL、容量、一次 pre-commit 明确拒绝恢复与跨 Share/用户/rail 隔离保持不变。凭据原文不存进 scope。固定代际 Bearer 切换的真实转发回环、迟到提交拒绝及 scope 每维区分单测已转绿。这是目标 Server 本地状态隔离，不推断新的 xAI wire；分次复核不是跨存储原子事务，离线测试不证明厂商接受性。

inference、media、remote_compaction 三项真实 receipt、GR-N4 同账号版本/header 接受性及当前未提交工作树的 committed-target observation 仍缺，继续独立 `live_pending`。最终整体 review 尚未完成，本轮不做 UI 自动化或部署测试；离线验证收口后下一类别为 Kiro。

验证收口：旧 HTTP 回环先复现同代次 Bearer 换用后错误注入 1 项 reasoning proof，修复后为 0；旧身份迟到提交在新身份下拒绝，scope 单测覆盖 Account incarnation 与请求身份摘要，摘要的调试输出脱敏。Grok 关键词 Rust library 199/199、相关集成 2/2；最终代码完整 Rust library 3331 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216` 串行运行，沿用既有 CodeBuddy 默认栈溢出的规避条件）。`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、Grok 私有 Node 159/159、全量 Node audit/smoke 1909/1909、本地服务 smoke 均通过；首次本地 smoke 因并行编译锁争用未启动，编译后单独重跑成功。Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引与差分空白检查通过；Phase 0 源码摘要由生成器刷新。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因明确跳过测试、缺凭据与未做部署验收返回预期 `blocked_inputs`，不能算发布通过。

### 18.156 Kiro SQLite 本地估算的持久账号实例隔离

2026-10-09，按循环只处理 Kiro。只读复核推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交 prompt-cache 实现；目标自身的本地 SQLite 估算已有请求凭据摘要，但同 ID、同 generation、同凭据删除重建后仍复用旧 namespace。定向测试在旧实现先红，证实旧估算可命中。现为 Kiro/Amazon Q AccountStore 账号持久化随机 128-bit owner ID：首次创建生成、普通 upsert/刷新/重启保留，在线删除重建和 ProviderType 切换轮换；生产 `prepare_bound_request` 把 owner ID 纳入缓存 namespace。旧账号文件无字段时按 ProviderType 与 ID 派生稳定迁移 ID，下次保存持久化。SQLite 重启保持合法 TTL 命中，账号重建后相同请求不读旧估算；生产绑定测试验证同一 Provider、session、凭据与两种代际下仅 owner ID 发生变化。owner ID 只用于本地计量隔离，不是出站凭据或上游 cache hit 证明；缓存失败仍不换号或改变请求成功条件。

无法从内容完全相同的离线手工重建旧 `accounts.json` 追溯删除事件；直接未绑定 AccountStore 的调用也只使用独立占位作用域。未推断新 EventStream wire，KI-N5 仍仅 shadow，Compact/shared cache 仍 disabled。八份真实 auth-kind×region receipt 各自 `live_pending`，当前未提交实现不产生 committed-target observation；最终整体 review 尚未完成，未做 UI 自动化或部署测试。离线验证收口后下一类别为 Qoder。

验证收口：账号 owner、旧存储迁移、SQLite 估算隔离与生产 `prepare_bound_request` 定向测试通过；完整 Rust 串行回归最终为 library 3335 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。首次与全量 Node 审计并跑时，既有 `accepted_provider_commit_survives_caller_cancellation` 时序测试失败；Node 完成后单测和完整 Rust 独立重跑均通过，本轮未改该测试。全量 Node audit 1896/1896、smoke Node 13/13、`scripts/static-checks.sh`、`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、本地服务 smoke、Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引与差分空白检查通过；Phase 0 源码摘要已由生成器刷新。八个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，没有厂商真实请求或 receipt。

### 18.157 Qoder 显式无效 usage 不得被本地补零伪装

2026-10-09，按循环只处理 Qoder。推荐 TokenRouter 当前 `AGENTS.md` 要求当前会话已加载 `project-doc`，但技能列表没有该项；不读取其业务源码，也不运行访问业务对象的 `--check-sources`，最新增量继续 `unreviewed_skill_gate`。读取门禁的同一次本地检查误附带只读 Git 状态/HEAD 命令，已停止后续外仓访问；不把这些元数据当作业务审计。本项依据目标 Server 自身计量正确性，不推断新 COSY wire。

旧 Qoder 响应兼容层把显式负数、对象或超范围 token 值当作缺失，再填 `0`；同组的优先别名有效时，备用别名无效也被跳过。固定上游 Chat chunk 的反例在旧实现先红。现在所有出现的 token 别名字段都须为可表示的非负计数，存在但无效即在 decoder 阶段按协议错误失败；直接进入非流聚合器的 Chat chunk 共用校验。既有数字字符串和有限小数截断兼容保持，缺失字段仍按原规则补零，合法 usage、唯一终态+EOF、请求 wire、站点/rail/Account/Share 绑定与 receipt schema v2/revision 2 不变。本地收紧只阻止伪计量，不证明厂商真实 usage 合同。Global OAuth、Global PAT、CN OAuth 的真实 receipt、登录/轮换、权威空目录、受控两段 401 和最新 TokenRouter 合规补审仍待完成；当前未提交工作树不能追加 committed-target observation。最终整体 review 未完成，未做 UI 自动化或部署测试。离线验证收口后下一类别为 CodeBuddy。

验证收口：无效 usage 定向反例先红后绿，Qoder 关键词 Rust 82/82；完整 Rust library 3336 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216` 串行运行）。`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、综合 `scripts/static-checks.sh`、Qoder receipt Node 281/281、本地服务 smoke、Qoder CLI oracle、自包含 reference-delta、Server Phase 0/coverage/UI matrix、文档索引及差分空白检查通过。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；离线 readiness 因显式跳过其内置测试、缺真实输入与未做部署验收而返回预期 `blocked_inputs`，不代表发布通过。

### 18.158 CodeBuddy finish_reason 与 DONE 之间的业务帧终态栅栏

2026-10-09，按循环只处理 CodeBuddy。只读核对推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 WorkBuddy SSE 样本：`finish_reason` 后是可选独立 usage 尾帧和 `[DONE]`，但其复制器本身不具备目标 Server 的严格终态合同；两个未跟踪文档排除。目标旧实现只禁止 `[DONE]` 后的业务数据，若上游先给 `finish_reason=stop`，随后再给新 content，最后补 `[DONE]` 与 EOF，仍可能把迟到内容纳入成功输出。decoder 与直接非流聚合的固定反例先红。

现在 CodeBuddy decoder 记住已出现的 finish-reason，在此前窗口继续允许 `choices:[]` usage 尾帧、无语义心跳和最终唯一 `[DONE]`；后续有语义 choice 立即按协议错误终止。非流聚合器共用同一边界。Intl/CN × Codex 流式回环确认晚到业务只产生失败终态、不产生 completed、不刷新账号，非流回环在响应提交前拒绝；usage 尾帧后再给业务同样拒绝。该改动不要求所有合法流都必须出现 finish-reason，保留既有 liveness 兼容样本；请求 URL/header/payload、单账号站点绑定、Share、缓存、唯一 `[DONE]`+EOF、receipt schema v2/revision 2 均不变。本地回环不能证明真实 Intl/CN 接受性，两站 receipt 和 CB-N4 `deepseek-v4.1-flash` 继续 `live_pending`/disabled；当前工作树未提交，不制造 committed-target observation。最终整体 review 尚未完成，未做 UI 自动化或部署测试。离线验证收口后下一类别回到 Antigravity。

验证收口：CodeBuddy 关键词 Rust 79/79；完整 Rust library 独立重跑为 3338 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216` 串行运行）。首次完整回归中与本轮 CodeBuddy 路径无关的 `api::accounts::tests::quota_waiter_reuses_quota_failure_instead_of_returning_stale_snapshot` 得到 502 而非预期 400；单独复测与完整重跑均通过，本轮未改该测试。`cargo check`、Clippy `--all-targets -D warnings`、rustfmt、`scripts/static-checks.sh`、CodeBuddy receipt Node 186/186、本地服务 smoke、CodeBuddy reference-delta `--check-sources`、Server Phase 0/coverage/UI matrix、文档索引与差分空白检查通过。Intl/CN 的 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；离线 readiness 因显式跳过其内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不能计为发布通过。

### 18.159 Antigravity 混合 SSE 换行时的 replay 事件切分

2026-10-09，按循环只处理 Antigravity。只读核对推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的冻结提交态；CLIProxyAPI 的 replay accumulator 按 SSE 行载荷观察响应，目标 Server 的事件切分属于自己的实现边界。旧 `next_sse_event` 在同一缓冲中优先搜索任意 `\r\n\r\n`，只有不存在时才搜索 `\n\n`。当签名内容以 LF 分隔、终态以 CRLF 分隔且同批到达时，旧代码错误地把两个 `data:` 事件并作一条，丢失签名和终态；固定回归先红。现按字节位置选择最早的完整分隔符，混合样本转绿，既有碎片 SSE 和 Antigravity replay 测试保持通过。这是本地捕获正确性修复，不变更下游响应、上游请求、Account/Share 绑定、AG-N6 requestType 或 compaction disabled 决策，也不把离线回归视作真实 Google 验收。

完整 Rust 独立串行回归为 library 3339 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）；Antigravity replay 定向 15/15。`cargo check`、综合 `scripts/static-checks.sh`（含 Clippy、rustfmt、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 检查）、本地服务 smoke、Antigravity reference-delta `--check-sources` 与差分空白检查通过。`docs/provider/coverage.md` 是生成文档，本轮覆盖矩阵无变化，只做 `--check`。两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；离线 readiness 因显式跳过其内置测试、缺真实输入和未做部署验收而返回预期 `blocked_inputs`，不能计为发布通过。两条真实 `antigravity_oauth` / `agy_oauth` receipt、AG-N6/AG-D3/compaction 专项证据及当前未提交工作树的 committed-target observation 仍缺；最终整体 review 未完成。未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.160 Claude SSE CR 与混合行结束符的双层解析一致性

2026-10-09，按循环只处理 Claude。只读核对推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 Claude translator/response 事件处理；它不定义本 Server 字节级 SSE 边界，也不作为新 Anthropic wire 接受性证据。目标 `AnthropicSseInspector` 原可认出 `\r\r` 边界，却只按 LF 拆字段；Claude OAuth 的工具名响应恢复器只识别 LF/CRLF 空行，CR-only 或混合换行时可能把事件留到 EOF 并将原 alias 返给下游。固定 CR-only 的 message lifecycle 与工具名事件、混合 LF/CRLF/CR 空行的两层反例先红。现由两层共用最早完整双行结束符的事件切分，检查器按 CR/LF 拆字段，恢复器按 CR/LF 定位单条 `data:` 并保留原始分隔字节。所有 chunk split、既有 LF/CRLF、多行 `data` 与原有事件生命周期测试保持通过。只修流解析和响应名恢复，不改变 Claude OAuth 请求/header、Share/Account 绑定、thread alias 状态、canonical plan 或 receipt schema。

完整 Rust 独立串行回归为 library 3343 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）；Anthropic 语义定向 11/11、Claude OAuth 定向 73/73。首次综合静态检查只因新增测试的一行 rustfmt 排版失败，格式化后完整重跑通过；`cargo check`、Clippy、全量 Node audit/smoke、Server Provider/Phase 0/coverage/UI matrix、文档索引、Web 检查、本地服务 smoke、Claude reference-delta `--check-sources`、离线 wire profile 与差分空白检查均通过。`docs/provider/coverage.md` 是生成文档，覆盖矩阵未变化，仅运行其 `--check`。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；离线 readiness 因显式跳过内部测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不能计为发布通过。五份真实 operation receipt、thread/beta 厂商接受性及当前未提交工作树的 committed-target observation 仍缺；最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

### 18.161 Codex Responses 同批重复 `[DONE]` 的唯一性栅栏

2026-10-09，按循环只处理 Codex。官方 OpenAI Docs Responses streaming 指南以 `response.completed` 为成功事件；只读复核推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 和 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`，两者不提供本 Server 的多租户终态记账与可选 `[DONE]` 唯一性规则。目标 Responses inspector 原先会在 `response.completed` 后重复重编码 `[DONE]`，使同批上游双尾标记可能被重复转发；固定 SSE/行分隔 JSON 单元反例先红。现第二次 `[DONE]` 立即协议失败；Codex HTTP stream 与非流聚合同批回环覆盖失败关闭和不写 completed。流式路径见终态便取消上游 body，不声称能观察取消后另批发来的重复标记。单个可选尾标记、仅有合法 `response.completed` 的成功流、请求/账号/Share 绑定与 receipt schema 不变；这是目标本地检查器的正确性修复，不是新厂商 wire 推断。五项真实 operation receipt、Astra/Image 2.5/WS prewarm 厂商接受性及未提交工作树的 committed-target observation 仍缺，保持独立 `live_pending`。最终整体 review 未完成，未做 UI 自动化或部署测试；本轮验证收口后下一类别为 Cursor。

验证收口：Codex 关键词 Rust library 413/413；完整 Rust 独立串行回归为 library 3346 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。`cargo fmt --check`、`cargo check`、综合 `scripts/static-checks.sh`（含 Clippy、Provider/Phase 0/coverage/UI matrix、文档索引、全量 Node audit 与 Web 源码测试）、本地服务 smoke、Codex reference-delta `--check-sources`、Provider profile/Phase 0/coverage 离线检查及差分空白检查均通过；生成的 `docs/provider/coverage.md` 未手工编辑。五项 `RUN_REAL=0` 各返回 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因明确跳过内置测试、缺真实凭据及未做部署验收而预期返回 `blocked_inputs`，不能算发布通过。下一单类别为 Cursor。

### 18.162 Cursor 续传缓存身份容量与工具 ID 唯一性

2026-10-09，按循环只处理 Cursor。只读复核推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 提交态的 parked h2 session/pending tool-call 映射；其 22 项未提交 Codex 内容排除，CUR-N2 `reviewed_no_wire_delta` 保持。目标自身 completed-response 缓存原只限制 response ID 为 512 字节，未限制保存的 conversation ID，也未将身份字符串计入单条/总容量；513 字节 conversation ID 在旧实现先红。现限制本地 conversation ID 至 512 字节，预算计入 response/conversation ID、scope 摘要和固定条目开销。目标 `surface_mcp_tool_call` 对已占用的上游 tool-call ID 原会覆盖 pending 映射，带首尾空白的 ID 则会被后续精确续接校验拒绝。现正常唯一 ID 原样保留，空白、超 512 字节或已占用的 ID 改为唯一的客户端 ID，同时保留各自 `exec_msg_id`/`exec_id`；定向回归覆盖实际工具映射，不改变 Cursor protobuf 或账号/Share/rail 绑定。这是本地状态和续接正确性修复，不推断新厂商 wire，也不把离线回环当真实 entitlement。OAuth/API Key 两份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺，保持独立 `live_pending`。最终整体 review 未完成，未做 UI 自动化或部署测试；验证收口后下一类别为 Grok。

验证收口：Cursor 关键词 Rust library 316/316；完整 Rust 独立串行回归为 library 3349 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。`cargo check`、rustfmt、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引和 Web 源码测试）、Cursor 私有 receipt Node 测试、本地服务 smoke、Cursor reference-delta `--check-sources`、Provider coverage/Phase 0 与差分空白检查均通过。文档中 SDK differential 脚本要求真实 `CC_SWITCH_SHARE_URL`，本轮尝试即在缺该输入处停止，未发请求，不能列为离线通过。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不代表发布通过。下一单类别为 Grok。

### 18.163 Grok SSE 搜索能力旁路证据的严格终态边界

2026-10-09，按循环只处理 Grok。只读核对推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 的已提交 Responses completed 样本及 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的终态处理；后者六项未提交 Codex 文件继续排除。目标主语义检查器早已拒绝冲突 SSE 声明，并不把 `response.done` 作为成功；Grok 搜索/质量旁路检查器却只读 `data` JSON，可能提前把冲突、伪 ping、未知字段、非终态 `response.done` 或失败状态 completed 记作成功证据，失败后迟到 completed 也能覆盖。三组定向反例在旧代码先红。现旁路对 SSE 字段、声明和 payload 类型做一致性检查；完成判断沿用主语义分类且 status 如存在须为 `completed`，首次终态后冻结旁路状态。Grok SSE 实际转发回环验证仅有 `response.done` 时主路径失败，搜索能力不持久化；正常 completed 与合法 `output_item.done` 的既有证据规则保留。不改变 xAI HTTP/WS/media 请求 wire、Grok OAuth Account/Share 绑定、GR-N4 版本/header 门槛或 receipt schema；离线反例不是厂商真实接受性。inference、media、remote_compaction 三份真实 receipt、GR-N4 同账号实测及当前未提交工作树的 committed-target observation 仍缺，保持独立 `live_pending`。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一类别为 Kiro。

验证收口：Grok 关键词 Rust library 241/241；完整 Rust 独立串行回归为 library 3353 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。`cargo check`、rustfmt、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 源码测试）、Grok 私有 receipt Node 测试、本地服务 smoke、Grok reference-delta `--check-sources`、Provider coverage/Phase 0 和差分空白检查均通过。综合静态检查第一次仅因新增测试使用单字符串 `concat!` 被 Clippy 拒绝，修正后完整重跑通过；未修改生产逻辑。inference、media、remote_compaction 三项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因明确跳过内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不能计为发布通过。下一单类别为 Kiro。

### 18.164 Kiro 本地 prompt-cache 签名的嵌套字段边界

2026-10-09，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐的干净 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`（tree `5d22f040f0e4b0aa775e61547540b3c040293cd8`）：参考 `src/anthropic/cache_metering.rs::without_cache_control` 只排除 block 顶层的断点声明。目标 `src/proxy/kiro_prompt_cache.rs` 原递归排除所有同名字段，使同一绑定下两份工具 schema 的 `input_schema/properties/cache_control` 内容虽不同，却错误命中同一 SQLite 本地估算前缀；固定反例先以非零 `cache_read_estimate` 红灯复现。现仅排除 block 顶层声明，嵌套字段保留并维持对象键稳定排序；缓存签名域从 v3 升至 v4，隔绝旧条目与新签名的潜在碰撞。不同 schema 首次 miss、同 schema 再次 hit 的反例已转绿。

本项只修本地 prompt-cache 计量估算，不改变实际发往 Kiro/Amazon Q 的请求、EventStream、Account/Share 绑定、KI-N5 usage authority 或真实上游计费；签名版本升级会让旧本地条目自然冷启动。完整 Rust 独立串行回归为 library 3354 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。`cargo check`、综合 `scripts/static-checks.sh`（含 Clippy、rustfmt、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引与 Web 源码检查）、Kiro reference-delta `--check-sources`、本地服务 smoke 和差分空白检查通过；综合静态检查首次只报新增代码格式，格式化后重跑通过。八个 auth-kind×region `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，无真实请求或 receipt；离线 readiness 因明确跳过内部测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。Compact/shared cache 继续 disabled，八份真实 receipt、KI-N5 专项证据、当前未提交工作树的 committed-target observation 与最终整体 review 仍缺。未做 UI 自动化或部署测试。下一单类别为 Qoder；其最新 TokenRouter 业务源码仍受当前会话缺失的 `project-doc` 技能门禁。

### 18.165 Qoder SSE 混合行结束符与唯一 `[DONE]` 边界

2026-10-09，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter；其 `AGENTS.md` 要求先调用当前会话不可用的 `project-doc` 技能，因此读取门禁后停止，没有读取业务源码或运行外仓 `--check-sources`。最新参考业务增量仍为 `unreviewed_skill_gate`。本项只依据目标 Server 的 `QoderSseDecoder`、`QoderChatSseAggregator` 与已冻结 CLI oracle，不推导新 COSY wire。旧两层解析器在一个缓冲里先找任意 CRLF 空行，再找 LF 空行；较早 LF 事件与较晚 CRLF 事件同批到达时会并成一条，报多 `data:` 错误。两层固定反例均先红；现按字节位置取最早完整空行，以 CR/LF 拆字段，LF、CRLF、CR-only 和逐字节分块回归转绿。非流聚合器原还重复接受 `[DONE]`，与原生 decoder 的唯一终态不一致；反例先红后绿，第二个标记即失败。

本项不改变 Qoder 请求 endpoint/header/payload、站点/rail、Account/Provider/Share 绑定、已有唯一终态+EOF 权威、receipt schema v2/revision 2 或三 rail 的真实状态。完整 Rust 独立串行回归为 library 3357 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）；`cargo check`、综合 `scripts/static-checks.sh`（含 Clippy、rustfmt、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 检查）、本地服务 smoke、自包含 Qoder reference-delta、CLI oracle 和差分空白检查通过。Global OAuth、Global PAT、CN OAuth 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；离线 readiness 因跳过内部测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不能算发布通过。三份真实 receipt、登录/轮换、权威空目录、受控两段 401、最新 TokenRouter 合规补审以及当前未提交工作树的 committed-target observation 仍缺；最终整体 review 未完成。未做 UI 自动化或部署测试。下一单类别为 CodeBuddy。

### 18.166 CodeBuddy 混合 SSE 空行的最早事件边界

2026-10-09，按循环只处理 CodeBuddy。`CodeBuddy/AGENTS.md` 推荐 `cli2api`；只读核对其 `a6502521e1e509a4257153025c642240c39cd54b` 已提交 WorkBuddy `sse.go` 逐行扫描逻辑，排除两个未跟踪文档。参考不定义本 Server 的跨网络块字节级空行判定。目标 `CodeBuddySseDecoder::next_event_boundary` 原只识别 LF+LF、CRLF+CRLF、CR+CR；一条业务事件以 `LF+CRLF` 结束、同批后面跟 `[DONE]` 时，旧实现将两条 `data:` 拼成一个 JSON payload 并报尾随内容。固定反例先红。现在顺序扫描最早的连续两个 CR/LF/CRLF 行结束符，任意字节分块下恢复原有 business 与唯一终态；双站 × Claude/Codex/Gemini × stream/non-stream 的十二路回环保留工具内容与 cache usage，均成功且未触发额外刷新。

这只修目标本地 SSE 事件切分，不推断厂商真实发送混合行结束符，也不改变 WorkBuddy 请求 endpoint/header/payload、Account/Provider/Share 绑定、finish-reason 后业务帧栅栏、唯一 `[DONE]`+EOF 或 receipt schema v2/revision 2。完整 Rust 独立串行回归为 library 3359 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。`cargo check`、综合 `scripts/static-checks.sh`（含 Clippy、rustfmt、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引与 Web 检查）、CodeBuddy reference-delta `--check-sources`、本地服务 smoke 和差分空白检查通过。Intl/CN 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；离线 readiness 因跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。两站真实 receipt、CB-N4 `deepseek-v4.1-flash` 厂商证据及当前未提交工作树的 committed-target observation 仍缺；最终整体 review 尚未完成，未做 UI 自动化或部署测试。下一单类别回到 Antigravity。

### 18.167 Antigravity replay 捕获器的 CR 与混合 SSE 空行

2026-10-09，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`：前者按已切出的 SSE 行观察 reasoning signature，后者的 pipeline/session 不定义目标 Server 的字节分帧；未从参考推断新 Google wire。目标 `AntigravityReplayStreamAccumulator` 原只认 LF+LF、CRLF+CRLF 空行，`observe_event` 只按 LF 拆字段。签名事件以 CR+CR、终态以 LF+CRLF 结束时，旧捕获器在同批输入下未能形成 replay 链，固定反例先红。现按最早连续的两个 CR/LF/CRLF 行结束符切事件，再按 CR/LF 拆字段；每个字节分块位置都能保留签名和终态，反例转绿。已有“终态帧到达即可提交 replay”的生命周期合同和测试保持不变，本轮不更改其时机。

本项只修本地 replay 捕获解析，不更改 Antigravity/AGY 出站请求、Account/Provider/Share 绑定、AG-N6 requestType、AG-D3/compaction、receipt schema v2/revision 2 或 compaction disabled 决策。完整 Rust 独立串行回归为 library 3360 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）；`cargo check`、综合 `scripts/static-checks.sh`（含 Clippy、rustfmt、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引与 Web 检查）、Antigravity reference-delta `--check-sources`、本地服务 smoke 和差分空白检查通过。`antigravity_oauth` 与 `agy_oauth` 两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；离线 readiness 因跳过内部测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。两份真实 receipt、AG-N6/AG-D3/compaction 专项证据和当前未提交工作树的 committed-target observation 仍缺，不能把混合换行 fixture 当作厂商接受性。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

### 18.168 Claude 成功响应的上游 model 字段栅栏

2026-10-09，按循环只处理 Claude。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 工作树干净，其 `claude_executor_stream.go::validateClaudeStreamingResponse` 明确拒绝缺 `id` 或 `model` 的 `message_start`。目标 `AnthropicSseInspector` 和非流式检查共用的 `validate_message` 原只检查 `id/role/content/usage`，上游缺失、空串或全空白 `model` 仍被视为成功。三种输入 × JSON/SSE 六个定向反例先红；现补齐非空 `model` 校验，HTTP 回环再验证 JSON/SSE 缺字段均在成功提交前返回 `502`。原有测试中省略字段的 Claude/Kimi 模拟响应补上模型，不更改真实出站请求、成功响应正文、Account/Provider/Share 绑定、CL-N7/CL-N8、receipt schema 或厂商权益判断。

本项只收紧 Anthropic-compatible 成功响应语义，不推导新 Anthropic wire，也不把本地 fixture 当作真实账号验收。五份 Claude operation receipt、续线程/beta 厂商接受性和当前未提交工作树的 committed-target observation 继续缺失；最终整体 review 未完成，未做 UI 自动化或部署测试。验证收口后下一单类别为 Codex。

验证收口：完整 Rust 独立串行回归为 library 3361 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）；`cargo check`、综合 `scripts/static-checks.sh`（含 Clippy、rustfmt、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 源码测试）、本地服务 smoke、Claude reference-delta `--check-sources`、离线 wire profile 与差分空白检查通过。完整回归首次仅有六个 library mock 和两个 API mock 因省略模型字段失败，补齐模拟上游响应后全绿；API 反例同时确认缺字段 JSON/SSE 均为 `502`。五个 `RUN_REAL=0` 入口分别为 `blocked_inputs/live_pending`，没有发起真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 返回预期 `blocked_inputs`（缺真实输入、明确跳过其内置测试及部署验收），不是发布通过。下一单类别为 Codex。

### 18.169 Codex completed 事件与显式状态的矛盾拒绝

2026-10-09，按循环只处理 Codex。`Codex/AGENTS.md` 推荐的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 当前工作树干净。后者的 Codex 测连诊断单测保留 `type=response.completed` 却带 `response.status=incomplete` 的矛盾样本；参考只作为风险线索，不作为官方协议认可。目标 `classify_value` 原仅凭事件名将其记为成功，`incomplete/in_progress/queued` 三种状态的 JSON/SSE 反例先红。现直接分类器不再将矛盾输入判为成功，普通 Responses JSON 与 SSE 守卫在成功提交前拒绝显式非 completed 状态；`null`、数字两种非字符串回归也失败关闭，缺 status 与合法 completed 的兼容正例保持通过。仅修本地终态语义，不改变 Codex 出站请求、Provider/Account/Share 绑定、Images 权限门槛、CX-N13/CX-D3 或 receipt schema。

本轮尝试按 OpenAI Docs 技能检索官方页面，但 `developers.openai.com` 与 `platform.openai.com` 均在 TLS 握手阶段失败；因此未据此新增官方 wire 结论。五项真实 Codex operation receipt、Astra/Image 2.5/WS prewarm 厂商接受性和当前未提交工作树的 committed-target observation 继续缺失；本地 fixture 不证明真实 entitlement。最终整体 review 未完成，未做 UI 自动化或部署测试。验证收口后下一单类别为 Cursor。

验证收口：完整 Rust 独立串行回归为 library 3362 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）；`cargo fmt --check`、`cargo check`、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引与 Web 源码测试）、本地服务 smoke、Codex reference-delta `--check-sources` 和差分空白检查通过。五个 `RUN_REAL=0` 入口分别为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因明确跳过内置测试、缺真实输入及未做部署验收而返回预期 `blocked_inputs`，不能计为发布通过。下一单类别为 Cursor。

### 18.170 Cursor 同身份作用域重复 ID 的 parked 会话歧义拒绝

2026-10-09，按循环只处理 Cursor。只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `cursorSessionManager.ts`：参考以 conversation ID 保存 parked h2 stream，并以待处理 tool-call ID 搜索；其 22 项未提交 Codex 内容排除。参考本身没有本 Server 的 Share/账号作用域，也不定义多会话重复 ID 的安全仲裁，本项不推断新 Cursor protobuf。目标 `CursorSessionManager` 的 scoped response/tool-call 索引原为单 binding；相同 scope 下两个不同会话绑定同一 ID 时后写覆盖，可能把旧续接路由到另一条 h2 stream。双索引固定反例先红。

现二级索引保留各个活跃候选，只有当前主 registry 中唯一有效会话时才返回 owner；歧义 ID 不再选后写者。重复绑定同一会话幂等，关闭碰撞会话后剩余唯一 owner 可恢复解析。Cursor session 定向 21/21 已通过；不改变出站请求、客户端 ID 生成、Account/Provider/Share/rail 绑定、同代次凭据隔离、completed Responses cache 或 receipt schema。离线反例不证明厂商真实会复用 ID，OAuth/API Key 两条真实 receipt 和未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试；验证收口后下一类别为 Grok。

验证收口：完整 Rust 独立串行回归为 library 3363 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）；`cargo fmt --check`、`cargo check -q`、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 源码测试）、本地服务 smoke、Cursor reference-delta `--check-sources` 和差分空白检查通过。OAuth/API Key 两个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，没有发起真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 返回预期 `blocked_inputs`（缺真实输入、明确跳过其内置测试及部署验收），不能计为发布通过。下一单类别为 Grok。

### 18.171 Grok Responses ping 帧的 SSE 重连元数据保留

2026-10-09，按循环只处理 Grok。只读对照 `Grok/AGENTS.md` 推荐的 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 ping 过滤器：它仅缓存单一 `event: ping` 后的 `data`/comment，遇到 `id`、`retry`、重复 `event` 或其他字段即原样回放；六项未提交 Codex 文件排除。`grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 提交态未变。目标原过滤器把 `event: ping` 加 `id` 的帧改成注释，丢失 SSE 重连 ID；`retry`、重复事件和未知字段同理。四类反例先红，现仅把单一、无有状态字段、无冲突业务类型的 ping 帧改注释，其余逐字节保留；所有二段切分、正常 ping/CRLF/上限回归覆盖。

本项只修下游 Grok Responses SSE 元数据保真，不改 xAI 出站 wire、Account/Provider/Share 绑定、搜索能力旁路、GR-N4 默认版本/group header、remote compaction disabled 或三项 receipt schema。参考项目的过滤规则不是厂商接受性的证明；inference/media/remote_compaction 真实 receipt、GR-N4 同账号版本/header 证据和当前未提交工作树的 committed-target observation 均缺。最终整体 review 未完成，未做 UI 自动化或部署测试；验证收口后下一类别为 Kiro。

验证收口：定向 Grok ping 过滤器 4/4，完整 Rust 独立串行回归为 library 3364 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）；综合 `scripts/static-checks.sh`（含 rustfmt、Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 源码测试）、Grok 私有 receipt Node 159/159、本地服务 smoke、Grok reference-delta `--check-sources` 和差分空白检查通过。三项 `RUN_REAL=0` 各返回 `blocked_inputs/live_pending`，没有发起真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因明确跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不能计为发布通过。下一单类别为 Kiro。

### 18.172 Kiro 本地 prompt-cache 全局选项键顺序规范化

2026-10-09，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的本地 prompt-cache 前缀签名/断点语义，不据此推断新的 Kiro EventStream wire。目标使用 `serde_json/preserve_order`，`global_cache_context` 原直接序列化 `thinking`、`output_config` 等嵌套 JSON 对象；同一 Share/账号/会话、完全相同的系统断点和选项值，仅对调嵌套字段顺序，第二次调用错误地 miss。定向反例先红。现只在全局上下文哈希前递归排序对象键；真实值改变仍 miss，数组顺序和值不变。此变更只影响本地估算签名，不改变出站 body、EventStream、Provider/Account/Share 绑定、权威 usage 或 v4 域；旧条目若非规范顺序会自然冷启动。

KI-N5 context usage 继续仅作 shadow，八份 auth-kind×region 真实 receipt 与独立厂商计量证据未齐前不切生产 usage authority；Kiro/Amazon Q Compact、shared cache 保持 disabled。当前未提交工作树不产生 committed-target observation。最终整体 review 未完成，未做 UI 自动化或部署测试；验证收口后下一类别为 Qoder。

验证收口：Kiro prompt-cache 定向 15/15；完整 Rust 独立串行回归为 library 3365 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。综合 `scripts/static-checks.sh`（含 rustfmt、Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 源码测试）、Kiro 私有 receipt Node 套件、本地服务 smoke、Kiro reference-delta `--check-sources` 和差分空白检查通过。八个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过其内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不能计为发布通过。下一单类别为 Qoder。

### 18.173 Qoder 上游错误码的安全回显边界

2026-10-09，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter；其 `AGENTS.md` 要求本会话不可用的 `project-doc` 技能，因此没有读取 TokenRouter 业务源码或运行其 `--check-sources`，最新参考增量仍为 `unreviewed_skill_gate`。只审目标 Server 与已冻结 CLI oracle：`QoderUpstreamError` 原把任意上游字符串 `code` 拼入 `ProxyError.message` 和下游 JSON，携带 Bearer 文本的定向反例先红。初版将代码缩至最多六位数字后再次复核发现，六位验证码形状仍可能泄露；最终仅保留本地用于权限/限额分类的 `112`/`115`，未知文本、数字、负数、小数及超长值不回显。上游 HTTP 状态与独立脱敏的 message 不变；未知代码失去诊断透出是刻意的安全兼容取舍。终态后空 JSON wrapper 的新增回归在旧实现已通过，不宣称该点先红后绿。

本项不改变 COSY 出站请求、三 rail 绑定、401 恢复次数、唯一终态+EOF 或 receipt schema v2/revision 2。本地 fixture 不能证明真实厂商错误码集合；Global OAuth、Global PAT、CN OAuth 三份真实 receipt、TokenRouter 最新业务源码合规补审与当前未提交工作树的 committed-target observation 继续缺失。最终整体 review 未完成，未做 UI 自动化或部署测试。验证收口后下一类别为 CodeBuddy。

验证收口：Qoder CLI oracle 与私有 receipt 离线 Node 套件 290/290；完整 Rust 独立串行回归为 library 3367 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。`cargo check -q`、`cargo fmt --check`、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 源码测试）、本地服务 smoke、自包含 Qoder reference-delta、CLI oracle 和差分空白检查通过；生成的 `docs/provider/coverage.md` 只核对未手改。三条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过其内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。

### 18.174 CodeBuddy 错误消息脱敏与未知业务码隔离

2026-10-09，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐的干净提交态 `cli2api@a6502521e1e509a4257153025c642240c39cd54b`：其 WorkBuddy `client.go::Classify` 为部分失败保留原始 body/`env.Msg`，不提供本 Server 的多租户错误展示安全合同；两个未跟踪文档排除，不运行或迁入参考代码。目标 `CodeBuddyUpstreamError` 原只截断上游 `msg/message`，含 Bearer 的 HTTP 错误会原样进入下游，定向反例先红。现复用本仓库敏感文本脱敏并过滤控制字符；递归业务码继续用于 HTTP/SSE 内部分类，仅本地已映射的认证、权限、限额或请求错误码可进入展示文本，未知数字不回显。已知 `11102/14003` 分类、HTTP 状态与一次同账号 pre-commit 401 恢复不变。未知码失去对外诊断数字是刻意安全取舍；无字段线索的任意秘密并不在通用脱敏保证内。

本项不改 WorkBuddy 请求 endpoint/header/body、Intl/CN 站点及 Account/Share 绑定、唯一 `[DONE]`+EOF、receipt schema v2/revision 2 或 CB-N4 disabled 决策。本地反例不证明真实厂商错误消息集合；两站真实 receipt、CB-N4 CN 厂商目录/接受性及未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。验证收口后下一类别回到 Antigravity。

验证收口：CodeBuddy decoder 定向 14/14；完整 Rust 独立串行回归为 library 3368 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。`cargo check -q`、`cargo fmt --check`、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 源码测试）、本地服务 smoke、CodeBuddy reference-delta `--check-sources`、Provider coverage 与差分空白检查通过。综合检查首轮仅因新增表达式未按 rustfmt 排版而停止；修正格式后完整重跑通过。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内部测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。

### 18.175 Antigravity/AGY 迟到容量响应的同代次 Bearer 隔离

2026-10-09，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`；CLIProxyAPI 的旧 token 目录探测 fencing 测试提供身份隔离风险线索，但不定义目标 Server 的 Google RPC 容量证据写入合同。本项不推断新 Google wire。目标 `record_limit_evidence` 原只用 `authIdentityGeneration` 栅栏：同 ID、同代际把 Bearer A 改成 B，A 的迟到 `MODEL_CAPACITY_EXHAUSTED` 仍可写进 B 的 `model_capacity`。状态层固定反例先红。

现在把本次请求 Account 快照和进程内 incarnation 传入状态域写入，在账号写锁内核对 Provider、auth/token generation、规范化 Bearer/relogin 身份摘要、project ID 与 incarnation。当前身份正常写入，旧身份、同 ID 重建、token generation 和项目漂移均拒绝。真实转发回环在 A 的 503 上游响应挂起时换为 B，确认不写入 B 的能力证据。本项只修本地观察隔离，不改变 Antigravity/AGY 出站 wire、Share/Account 路由、同账号重试、AG-N6 requestType、AG-D3/compaction、receipt schema v2/revision 2 或 disabled 决策。离线 fixture 不证明 Google 接受性；两条真实 rail receipt、AG-N6/AG-D3 专项证据与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Claude。

验证收口：状态定向与真实转发回环均通过；完整 Rust 独立串行回归为 library 3370 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt Node 165/165；`cargo check -q`、`cargo fmt --check`、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引和 Web 检查）、本地服务 smoke、Antigravity reference-delta `--check-sources`、Provider coverage 与差分空白检查通过。Phase 0 生成器已刷新状态 writer 源码摘要，生成的 `docs/provider/coverage.md` 仅核对未手改。`antigravity_oauth`、`agy_oauth` 两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发起真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过自身内置测试、缺真实输入且未做部署验收返回预期 `blocked_inputs`，不是发布通过。

### 18.176 Claude SSE 缺失 message_delta 的假成功拒绝

2026-10-09，按循环只处理 Claude。只读复核 `Claude/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：其 `validateClaudeStreamingResponse` 与 `TestClaudeExecutor_ExecuteOpenAINonStreamRejectsIncompleteClaudeStream` 明确将只有 `message_start`、`message_stop` 而没有 `message_delta` 的流视为未完成。目标 Server 的 `AnthropicSseInspector` 原只要 start/stop 就标成功；定向反例先红。现成功 `message_stop` 之前必须有至少一条经 usage 校验的 `message_delta`，否则失败关闭。Claude OAuth HTTP 回环验证同批不完整流在提交成功前返回 502、不产生 completed usage；正常含 delta 的结束仍及时取消上游 body。已有错误事件、内容块、混合 SSE 换行合同不变。

本项只修 Claude 响应终态，不改变出站 Messages/count_tokens、单账号 Provider/Share 绑定、401 恢复、静态模型目录、receipt schema 或模型 entitlement。参考及本地回环不能代替真实 Anthropic 接受性；五项 operation receipt、续线程/beta 厂商证据和当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试。下一单类别为 Codex。

验证收口：新反例先红后绿，Anthropic inspector 定向 13/13，Claude HTTP 回环确认缺 delta 的同批响应在成功提交前返回 502。完整 Rust 独立串行回归为 library 3372 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 252/252；`cargo check -q`、`cargo fmt --check`、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引和 Web 检查）、本地服务 smoke、Claude reference-delta `--check-sources`、Provider coverage 与差分空白检查通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过自身内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。

### 18.177 Codex Responses 完成项 ID 与非空终态 output 保真

2026-10-10，按循环只处理 Codex。官方 OpenAI Docs 的 Responses streaming 指南确认 `response.completed` 是成功完成事件，不承诺订阅账号权限。只读核对 `Codex/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`；前者的 `codex_executor_terminal.go` 在非空终态 output 中按已完成项索引补缺失 ID，并将无索引项分开收集。目标 HTTP SSE/WebSocket patcher 原只修空 output，非空 output 缺 ID 的双路径反例先红；目标非流聚合还会用完成项整体覆盖非空终态 output，且有索引项之后的无索引项错误复用索引 0，两条反例也先红。

现把只补缺失、`null` 或空白 ID 的规则收敛为共用辅助函数，按对应 `output_index` 取先前完成项；已有 ID、终态 output 的工具参数和其他字段不覆盖。非流聚合只在终态 output 为空时重建，显式索引项排序后再追加无索引项，避免碰撞；Codex OAuth 非流 HTTP 回环验证完整返回。HTTP SSE、WebSocket 与非流聚合均有定向回归。本项不变更出站 Responses/Images wire、Account/Provider/Share 单绑定、WS→HTTP 恢复、receipt schema 或 CX-N13/CX-D3 决策。离线回环不能证明 ChatGPT 实际模型/图片权限；五项真实 operation receipt、最新厂商接受性与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试。下一单类别为 Cursor。

验证收口：四组定向反例先红后绿；HTTP SSE 与 WebSocket 补 ID、非流保留非空终态 output、索引/无索引拼接均通过，Codex OAuth 非流 HTTP 回环也通过。完整 Rust 独立串行回归为 library 3377 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205；`cargo check -q`、`cargo fmt --check`、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 检查）、本地服务 smoke、Codex reference-delta `--check-sources`、Provider coverage 与差分空白检查通过。首次本地 smoke 因 Cargo 编译/锁等待超过脚本的 30 秒启动窗口未连上服务；预构建后原样重跑通过。中断的增量编译曾留下全零测试二进制并触发 rustc 1.94.1 增量缓存内部错误，后续以 `CARGO_INCREMENTAL=0` 完成回归，损坏的生成产物已移除。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过自身内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。

### 18.178 Cursor 冷续接快照失败不得占用消费 ID

2026-10-10，按循环只处理 Cursor。只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 Cursor 会话管理与 executor：参考在同一 h2 流成功发送工具结果后删除 pending 映射；其 22 项未提交 Codex 工作树内容排除。参考不定义本 Server 的冷续接内存预算与多租户 claim，因此本项是目标自身的失败边界修复，不推断新的 Cursor protobuf 或厂商 wire。

目标 Responses 冷续接原在取得 parked 会话后立即 claim 工具/响应 ID，再复制早前语义上下文并预留请求内存。固定 1 字节预算反例先红：快照失败时原会话由 guard 关闭，但 ID 仍被标记已消费，下一请求被错误判为重复续接。现仅在快照与内存预留全部成功后、关闭原 parked 会话前写 claim；成功路径的并发重复开流栅栏保留，失败路径不再污染 scope 隔离的 claim 集。既有 OAuth/API Key 身份、Share 作用域、TTL/容量、出站 AgentService 请求与 receipt schema 不变。离线测试不证明真实 Cursor 接受性，两条 rail 的真实 receipt 及当前未提交工作树的 committed-target observation 继续 `live_pending`；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一单类别为 Grok。

验证收口：新反例先红后绿；完整 Rust 独立串行回归为 library 3378 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Cursor 私有 receipt Node 95/95；`cargo check -q`、`cargo fmt --check`、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 源码检查）、本地服务 smoke、Cursor reference-delta `--check-sources`、生成 coverage 核对和差分空白检查通过。两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因明确跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。

### 18.179 Grok Responses SSE 混合空行与超大帧直通恢复

2026-10-10，按循环只处理 Grok。`Grok/AGENTS.md` 推荐的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 工作树干净；`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 六项未提交 Codex 文件继续排除。只读核对后者已提交 Grok ping 过滤器的逐行扫描：CR、LF、CRLF 均为行结束，块末尾 CR 暂缓到下一块判定。参考没有定义本 Server 的搜索/质量旁路、Share 身份或 receipt，不据此推断新 xAI wire。

目标 `GrokResponsesSseInspector` 原只识别 LF+LF 与 CRLF+CRLF 空行。混合 LF+CRLF、CRLF+LF 及 CR-only ping 在所有字节切分下不能及时规范化，完成搜索的混合帧也不会及时产生旁路观察；固定反例先红。现按最早的两次逻辑行结束分帧，并在第二个 CR 恰处于块末时等待下一块，避免将跨块 CRLF 的 LF 留给下一帧。字段解析兼容 CR-only；超大帧直通结束也按相同边界恢复后续 ping/搜索帧。合法完整事件的原始字节继续保留，只有原本允许的无状态 ping 转成注释。主 Responses 成功终态、搜索证据持久化门槛、Grok OAuth Account/Share 绑定、上游请求、GR-N4 默认版本/group header、媒体与 remote compaction disabled 决策和 receipt schema 均不变。离线换行反例不能证明真实 xAI 发送该格式；inference、media、remote_compaction 三份真实 receipt、同账号版本/header 证据与未提交工作树 committed-target observation 仍各自 `live_pending`。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Kiro。

验证收口：混合/CR-only 分帧反例先红后绿，相关 Grok SSE 定向 7/7；完整 Rust 独立串行回归为 library 3380 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 私有 receipt Node 159/159；`cargo check -q`、rustfmt、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引及 Web 源码检查）、预构建后的本地服务 smoke、Grok reference-delta `--check-sources`、生成 coverage 核对与差分空白检查通过。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因明确跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。

### 18.180 Kiro EventStream 上游错误诊断隔离

2026-10-10，按循环只处理 Kiro。只读复核 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 EventStream `error`/`exception` 解析：参考保存 header 错误码和 payload 诊断供内部处理，不定义本 Server 多租户客户端的展示边界。本轮不推断新 EventStream wire 或厂商计量权威。

目标 `wire::parse_event` 能正确识别上游错误帧，但 `KiroToolJsonError::from` 原把不可信 code/message 通过 `WireError::to_string()` 放入下游非流及流式错误。两种帧各以 header、payload Bearer 文本构造反例，旧实现原样回显，先红。现仅对上游 error/exception 转换为稳定的本地 `KIRO_UPSTREAM_STREAM_ERROR` 与通用消息；本地 EventStream 结构、容量和超时错误仍沿用既有说明。保留上游状态失败分类，不改 Kiro/Amazon Q 出站请求、解码/终态、Account/Provider/Share 和 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema。离线注入不代表真实厂商接受性，八份真实 receipt、KI-N5 专项证据和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Qoder，其推荐 TokenRouter 业务源码继续受 `project-doc` 技能门禁。

验证收口：两类反例先红后绿；完整 Rust 独立串行回归为 library 3381 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Kiro 私有 receipt Node 492/492；`cargo check -q`、rustfmt、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引与 Web 源码检查）、预构建后的本地服务 smoke、Kiro reference-delta `--check-sources`、生成 coverage 核对与差分空白检查通过。八个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因明确跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。

### 18.181 Qoder SSE 显式状态的失败关闭

2026-10-10，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter；该仓 `AGENTS.md` 要求当前会话先调用不可用的 `project-doc` 技能。读取门禁后不读其业务源码、不运行外仓源码审计，最新参考增量继续 `unreviewed_skill_gate`。本轮只对照目标 `QoderSseDecoder`、本地 fixture 和已冻结 Qoder CLI oracle，不推导新 COSY wire。

目标 SSE wrapper 原把显式无效 `statusCodeValue`/`statusCode` 当作缺失并默认 200；若两字段矛盾也直接采用前者。负数、空值、未知名称、错误类型、越界状态与 200/403 冲突的固定反例先红。现每个出现的状态字段都须解析为 100～599 的 HTTP 状态，两字段同时出现须一致；数字字符串和 `OK` 等成功兼容形式以及完全缺失状态时的默认 200 保持可用。无效状态在 decoder 阶段以固定本地协议错误失败，不回显原始值。此项不改 COSY 出站 endpoint/header/body、Global/CN 站点与 OAuth/PAT rail、Account/Provider/Share 绑定、唯一终态+EOF、receipt schema v2/revision 2 或 401 预算。离线反例不证明厂商真实状态集合；Global OAuth、Global PAT、CN OAuth 三份真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审和当前未提交工作树 committed-target observation 均待完成。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 CodeBuddy。

验证收口：显式无效状态反例先红后绿，字符串 `401` 被归类为上游 401，冲突状态为本地协议错误。完整 Rust 独立串行回归为 library 3382 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）；Qoder 私有 receipt Node 281/281、冻结 CLI oracle、自包含 reference-delta、生成 coverage 核对、文档索引与差分空白检查通过。`cargo check -q`、rustfmt、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引和 Web 源码检查）通过。预构建后的本地服务 smoke 首次因运行时未继承 `CARGO_INCREMENTAL=0` 而在 30 秒窗口内重新编译、健康连接失败；以相同构建环境重跑后通过健康、登录、Provider 与 Share 创建，未将首次失败隐藏。三个 `RUN_REAL=0` 入口分别仍为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.182 CodeBuddy SSE 终止原因类型一致性

2026-10-10，按循环只处理 CodeBuddy。`CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 WorkBuddy `sse.go::Aggregate` 将 `finish_reason` 解作字符串；参考的宽松流复制器不定义目标 Server 的严格多租户终态合同。参考工作树两个未跟踪文档继续排除。本轮不推断新的厂商 wire。

目标非流聚合器原已要求 `finish_reason` 为非空字符串或 `null`，但流式 decoder 接受数字、布尔值或对象并转发，下游可在这些伪终止值后继续收业务帧，再以 `[DONE]`+EOF 写成功。固定反例在旧实现先红。现于流式规范化前拒绝显式非字符串且非 `null` 的值；保留 `null`、空白字符串作为原有可丢弃心跳形式，以及合法非空字符串后的独立 usage 尾帧。原有 finish 后业务帧栅栏、唯一 `[DONE]`+EOF 和非流严格解析保持。此项不改 WorkBuddy 请求 endpoint/header/body、Intl/CN 站点、Account/Provider/Share 绑定、receipt schema v2/revision 2 或 CB-N4 disabled 决策。离线反例不是两站真实验收；Intl/CN receipt、CN `deepseek-v4.1-flash` 厂商目录/接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一类别回到 Antigravity。

验证收口：异常终止原因反例先红后绿，CodeBuddy 定向 Rust 83/83；完整 Rust 独立串行回归为 library 3383 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量 audit Node 1896/1896 与 smoke Node 13/13、冻结 reference-delta `--check-sources`、生成 coverage 核对、文档索引及差分空白检查通过。`cargo check -q`、rustfmt、综合 `scripts/static-checks.sh`（含 Clippy、Provider/Phase 0/coverage/UI matrix 与 Web 源码检查）及预构建后的本地服务 smoke 通过。Intl/CN 两个 `RUN_REAL=0` 入口分别仍为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.183 Antigravity replay 终态后的签名捕获栅栏

2026-10-10，按循环只处理 Antigravity。推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 仍只作只读参考。CLIProxyAPI 的 replay accumulator 允许捕获携带 `finishReason` 的同一终态帧签名，但不提供本 Server 所需的同块后置事件隔离；Manager 也不定义这项 replay 字节边界。本轮不推导新的 Google wire。

目标本地 `AntigravityReplayStreamAccumulator` 原在 `finishReason` 后仍把同网络块后续带签名业务帧并入重放链，`SIG_A` 正常终态后附 `SIG_B` 的固定反例先红。现终态帧自身内容照常进入 replay；后续 SSE 事件与网络块不再改写已完成轮次，避免终态后的未授权签名进入下次请求。主数据面终态判定、下游输出与错误语义不在此项改动范围，不能把旁路栅栏当作主流严格验证。此项不改变 Antigravity/AGY 出站请求、Account/Provider/Share 绑定、AG-N6 requestType、AG-D3/compaction、receipt schema v2/revision 2 或 disabled 决策。两条真实 OAuth rail receipt、AG-N6/AG-D3 专项接受性及当前未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一类别为 Claude。

验证收口：终态后签名注入的同块/分块反例先红后绿，Antigravity 定向 Rust 93/93；完整 Rust 独立串行回归为 library 3384 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt Node 165/165、全量 audit Node 1896/1896 与 smoke Node 13/13、冻结 reference-delta `--check-sources`、生成 coverage 核对、文档索引及差分空白检查通过。`cargo check -q`、rustfmt、综合 `scripts/static-checks.sh`（含 Clippy、Provider/Phase 0/coverage/UI matrix 和 Web 源码检查）及预构建后的本地服务 smoke 通过。两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.184 Claude message_delta 的结构性成功门槛

2026-10-10，按循环只处理 Claude。推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `validateClaudeStreamingResponse` 已要求存在 `message_delta`，已提交样本含对象 `delta`，但其校验器不定义目标 Server 的完整结构合同。本轮只修目标自身已落地的响应语义检查，不推断新 Anthropic wire。

目标 `AnthropicSseInspector` 原只要 `message_delta.usage.output_tokens` 合法，就把缺 `delta` 或 `delta` 为 `null`、字符串、数组的伪事件算作完成进度；随后 `message_stop` 可得到成功终态。固定反例先红。现 `message_delta` 必须显式携带对象 `delta`；空对象 `{}` 继续可用，原有 usage 校验、内容块、错误帧和 start/delta/stop 时序保持。此项不改 Claude OAuth 出站 Messages/count_tokens、续线程 CL-N7、`pause_turn` CL-N8、Account/Provider/Share 绑定、静态目录、receipt schema 或 entitlement 决策。离线结构反例不能证明真实账号接受性；五份 operation receipt、thread/beta 厂商证据及当前未提交工作树 committed-target observation 均缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一类别为 Codex。

验证收口：缺失/错误类型的 `delta` 反例先红后绿，Anthropic 语义定向 Rust 14/14、Claude 相关库测试 288/288；完整 Rust 独立串行回归为 library 3385 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 252/252、全量 audit Node 1896/1896 与 smoke Node 13/13、冻结 reference-delta `--check-sources`、离线 wire profile、生成 coverage 核对、文档索引及差分空白检查通过。`cargo check -q`、rustfmt、综合 `scripts/static-checks.sh`（含 Clippy、Provider/Phase 0/coverage/UI matrix 和 Web 源码检查）及预构建后的本地服务 smoke 通过。五项 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.185 Codex Responses 同批冲突终态失败关闭

2026-10-10，按循环只处理 Codex。只读对照 `Codex/AGENTS.md` 推荐、工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`。官方 OpenAI Docs 的 Responses streaming 指南和事件参考确认 `response.completed` 表示模型响应完成，但不证明私有 ChatGPT 订阅账号权限；本项仅修目标 Server 的本地成功判定边界，不推断新上游 wire。

目标流式 `ResponsesSseInspector` 和非流 `ResponsesSseAggregator` 原在识别首个终态后忽略同批后续 JSON。成功 `response.completed` 后附失败、不完整、重复完成或 `error` 的四类固定反例先红。现两处共用终态识别规则：后续声明事件名或 payload 指示任何终态/错误帧，都以固定本地协议错误失败；普通后置 JSON 和唯一可选 `[DONE]` 仍沿用原有处理，不泄露上游错误正文。流式转发在首批确认终态后可主动取消 body，因此仅保证已解码批次内的冲突检测，不声称能观察后续网络批次。本项不变更出站 Responses/Images wire、WebSocket、Account/Provider/Share、Images/预热启用状态或 receipt schema。五项真实 Codex operation receipt、厂商接受性以及当前未提交工作树的 committed-target observation 均缺；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一单类别为 Cursor。

验证收口：四类冲突终态在流式检查与非流聚合两处定向反例先红后绿；全量 Rust 首轮发现一条仍断言“完成后忽略失败”的旧 line-delimited JSON 测试，与新边界矛盾，已改为拒绝断言后重跑通过。完整 Rust 独立串行回归为 library 3387 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205、综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引和 Web 检查）、Codex reference-delta `--check-sources`、`cargo fmt --check`、`cargo check -q`、预构建后的本地服务 smoke 与差分空白检查通过。五项 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.186 Cursor 活跃 h2 会话硬容量门槛

2026-10-10，按循环只处理 Cursor。只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 session manager：默认限制 100 个会话，超限时驱逐最早活动者；22 项未提交 Codex 内容排除。参考会驱逐运行中会话，但目标需要保护已开 AgentService 流，只采用容量边界信号，不照搬该驱逐策略或推断新 Cursor wire。`CUR-N2` 的 no-new-wire 结论保持。

目标 `CursorSessionManager::reserve` 原在插入后只驱逐非运行中会话，全部为 `Running` 时无候选，新请求仍成功返回，默认 100 会话上限可无限突破。上限 1、两个不同运行会话的反例先红。现停放会话仍可按既有最早活动策略让位；若新入口插入和驱逐后仍超限，在入口逃逸给调用者或打开上游 h2 前关闭并删除新入口，返回固定、可重试的本地 `503 cursor_session_capacity_exhausted`，不打断已有运行流。此项只修本地 admission；不改变 OAuth/API Key、Account/Provider/Share 身份绑定、请求/响应 protobuf、续接索引与 claim、receipt schema v2/revision 2。离线容量反例不证明真实 Cursor 接受性；OAuth/API Key 两条真实 receipt 与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试；验证收口后下一单类别为 Grok。

验证收口：运行中会话硬上限反例先红后绿，停放会话可驱逐正例与 503 稳定错误码/一秒重试提示回归通过。完整 Rust 独立串行回归为 library 3389 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Cursor 私有 receipt Node 95/95；综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引和 Web 检查）、Cursor reference-delta `--check-sources`、生成 coverage 核对、`cargo fmt --check`、`cargo check -q`、预构建后的本地服务 smoke 与差分空白检查通过。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.187 Grok 推理证明流式捕获的终态后栅栏

2026-10-10，按循环只处理 Grok。只读复核 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 和 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交状态；后者六项未提交 Codex 内容排除。前者质量扫描器将 completed/incomplete/failed 设为终态，后者 SSE 过滤器只处理 ping 兼容；参考没有本 Server 的多租户 reasoning proof 缓存提交合同。本轮不推断新的 xAI wire，GR-N4 同账号版本/header 接受性继续 `live_pending_no_wire_change`。

目标 `GrokReplayStreamAccumulator` 原在 completed 后仍接收额外 output item，并忽略前后出现的 `error` 帧；异常流可产生下一轮复用的 opaque proof。完成后迟到 output/error、先 error 后 completed 的固定反例先红。现错误、失败、不完整或取消终态一律废弃候选，成功终态后任何额外 JSON 也废弃；单个可选 `[DONE]` 只能在完成后出现，过早或重复均拒绝。正常分片 SSE、WebSocket JSON 和唯一结束标记保持可用。此项仅修旁路缓存，主数据面的成功/失败仍由主语义检查器判定；不改 Grok HTTP/WS/media 出站、Account/Provider/Share 绑定、默认 CLI 版本、group header、remote compaction disabled 或三项 receipt schema。离线反例不能证明真实 xAI 行为；inference/media/remote_compaction receipt、GR-N4 同账号实测和当前未提交工作树的 committed-target observation 继续缺失。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Kiro。

验证收口：完成后迟到 output/error、错误后补完成两组反例先红后绿；扩展覆盖声明式 error、cancelled/incomplete、唯一/过早/重复 `[DONE]`，正常 SSE/WebSocket 捕获仍通过。完整 Rust 独立串行回归为 library 3392 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 私有 receipt Node 159/159；综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引和 Web 检查）、Grok reference-delta `--check-sources`、生成 coverage 核对、rustfmt、`cargo check -q`、预构建后的本地服务 smoke 和差分空白检查通过。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.188 Kiro EventStream 终态 payload 与解码失败状态一致性

2026-10-10，按循环只处理 Kiro。只读复核 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交帧解码器和事件解析：参考逐帧处理、限制缓冲并维护解码状态，不定义目标 Server 的 `endEvent` 成功证据。本轮只收紧目标自身已声明的严格终态合同，不推断新的 Kiro/Amazon Q wire。

目标 `wire::parse_event` 原无条件把 `endEvent` 当成功，即使完整 CRC 帧的非空 payload 是畸形 JSON；`EventStreamDecoder::finish` 原未先检查 `feed` 已记录的失败，在容量错误未追加字节时可返回成功。两条反例先红。现空 payload 和合法 JSON payload 的 `endEvent` 保持可用；非空畸形 payload 按本地 `KIRO_EVENT_STREAM_INVALID` 失败，`finish` 返回先前的结构或容量错误。流式和非流式转换复用同一解析器；不改出站请求、Account/Provider/Share 与 auth-kind×region 绑定、KI-N5 context usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线反例不能证明真实上游发送畸形帧；八份真实 receipt、KI-N5 专项厂商证据和当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Qoder，其推荐 TokenRouter 业务源码仍需遵守 `project-doc` 技能门禁。

验证收口：畸形非空 `endEvent` payload 与容量错误后 `finish` 洗白两条反例先红后绿，合法 `{}`/空 payload 回归通过，EventStream wire 定向 10/10。完整 Rust 独立串行回归为 library 3394 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Kiro 私有 receipt Node 492/492；综合 `scripts/static-checks.sh`（含 Clippy、全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引和 Web 检查）、Kiro reference-delta `--check-sources`、生成 coverage 核对、rustfmt、`cargo check -q`、预构建后的本地服务 smoke 与差分空白检查通过。八个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.189 Qoder Chat choice 与终止原因的流式失败关闭

2026-10-10，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter；该仓 `AGENTS.md` 要求先调用当前会话不可用的 `project-doc` 技能。本轮没有读取其业务源码或运行外仓源码审计，最新参考增量继续 `unreviewed_skill_gate`。只核对目标 `QoderSseDecoder`、`QoderChatSseAggregator` 与已冻结的 Qoder CLI oracle，不推断新 COSY wire。

旧流式 decoder 会把不可解析的 `choices[].index` 默认为 0，并在转发前放过布尔或空白 `finish_reason`；非流聚合器虽拒绝布尔终止原因和非零 index，却同样默许不可解析 index 回退为 0。布尔/空白终止原因与非零/不可解析 index 的固定反例先红。现 decoder 转发前验证每个 choice 的 index 和显式终止原因，两层共用 index 0 校验；缺失 index 仍默认 0，合法同帧业务+终态与唯一终态+EOF 保持。这是目标本地协议完整性修复，不改变 COSY 出站请求、Global/CN 站点、OAuth/PAT rail、Account/Provider/Share 绑定、receipt schema v2/revision 2 或一次同账号 401 预算。离线反例不证明真实厂商选择项格式；Global OAuth、Global PAT、CN OAuth 三份真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 CodeBuddy。

验证收口：上述定向反例先红后绿，最终 Qoder 定向 Rust 88/88。完整 Rust 独立串行回归在最后一处等价 Clippy 写法调整前为 library 3394 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）；等价调整后的定向测试、`cargo check -q`、rustfmt 与综合 `scripts/static-checks.sh` 均通过。静态检查首轮只因新增布尔表达式的 Clippy `nonminimal_bool` 提示失败，改写后全套重跑通过，含全量 Node audit、Provider/Phase 0/coverage/UI matrix、文档索引和 Web 源码检查。Qoder 私有 receipt Node 281/281、冻结 CLI oracle、自包含 reference-delta、生成 coverage 核对、预构建后的本地服务 smoke 与差分空白检查通过。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过其内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.190 CodeBuddy 混合业务错误帧优先于 Chat choice

2026-10-10，按循环只处理 CodeBuddy。只读复核 `CodeBuddy/AGENTS.md` 推荐、工作树仅有两个无关未跟踪文档的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 WorkBuddy 实现：`client.go::Classify` 识别非零业务码，`sse.go` 是宽松流复制/聚合，不定义本 Server 的严格多租户终态。本轮只修目标自己的错误优先级，不推断新的上游 SSE wire。

目标 `CodeBuddyUpstreamError::from_event` 原只要帧带 `choices` 字段就跳过已识别的业务错误码。固定 `14003` 与空 choices 混合，首个 Intl 非流回环误返回普通 502；改用完整 content/finish choice 并随后发送 `[DONE]`，旧非流路径错误返回 200。两个反例先红。现非零业务码在 decoder 转发前优先于 choices，直接非流聚合器共用该分类；保留 `14003` 的 429 限流语义，不误刷新账号。`code=0` 正常帧、Intl/CN 出站 endpoint/header/body、Account/Provider/Share 绑定、独立 usage 尾帧、唯一 `[DONE]`+EOF、receipt schema v2/revision 2 与 CB-N4 disabled 决策不变。离线混合帧不能证明厂商真实发送该形状；Intl/CN 两份真实 receipt、CN `deepseek-v4.1-flash` 厂商证据及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别回到 Antigravity。

验证收口：混合错误帧单元与 Intl/CN × 流/非流回环先红后绿，相关 `business_error` 定向 Rust 4/4；完整 Rust 独立串行回归为 library 3396 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo check -q`、rustfmt、Clippy `--all-targets -D warnings`、CodeBuddy 私有 receipt Node 186/186、全量 Provider/Smoke Node 1909/1909、CodeBuddy reference-delta `--check-sources`、生成 coverage、Server Provider/Phase 0/UI matrix 静态审计、文档索引、本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 会运行 Web UI 测试；本轮遵守禁止 UI 自动化测试的要求，改为逐项运行所需后端/合同检查，未做该脚本中的 Web 测试。Intl/CN 的 `RUN_REAL=0` 分别仍为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.191 Antigravity replay 按未完成事件限制缓冲

2026-10-10，按循环只处理 Antigravity。只读复核推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：其 replay accumulator 以 `ObserveSSELine` 逐行观察输出，不将一次网络 read 的总量等同于单条事件；另一推荐 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 仍作为已有差分参考，但本项不从它推导目标 Server 的 SSE retained-buffer 容量。只借鉴事件粒度，不推断新 Google wire。

目标 replay 旁路旧实现在拆分事件之前先检查整个网络块是否超过 16 MiB。两条各约 8 MiB 的完整注释事件和一个签名终态同批到达时，旧实现错误丢弃 replay；固定反例先红。现按未完成事件分段缓存，及时消费完整事件，超限的单个未完成事件仍丢弃候选，终态后的事件仍不可改写已完成轮次。大突发排空后释放闲置容量，避免后续请求内存长期被无效占用。这只修 replay 捕获，不改变主数据面成功判定、Antigravity/AGY 出站 wire、Account/Provider/Share 绑定、AG-N6 requestType、AG-D3/compaction、receipt schema v2/revision 2 或 disabled 决策。离线大块 fixture 不证明真实 Google 上游如何分块；两条真实 OAuth receipt、AG-N6/AG-D3 专项厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Claude。

验证收口：大块多事件反例先红后绿，replay stream-accumulator 定向 4/4，原有超限清空和碎片终态测试保持通过。完整 Rust 首轮 library 3396 passed / 1 failed / 1 ignored，唯一失败是既有 Account quota waiter 时序测试（期望 400、得到 502），与 replay 路径无关；该测试独立复测通过，随后完整 Rust 独立重跑为 library 3397 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo check -q`、rustfmt、Clippy `--all-targets -D warnings`、Antigravity 私有 receipt Node 165/165、冻结 reference-delta `--check-sources`、生成 coverage、Server Provider/Phase 0/UI matrix 静态审计、文档索引、本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 会触发 Web UI 测试，本轮按禁止 UI 自动化测试要求只运行相关后端和合同检查。`antigravity_oauth` / `agy_oauth` 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.192 Claude 已知内容块增量载荷的结构性成功门槛

2026-10-10，按循环只处理 Claude。只读复核 `Claude/AGENTS.md` 推荐、工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：其 Anthropic→OpenAI Responses/Chat 转换器按类型读取 `text_delta.text`、`thinking_delta.thinking`、`signature_delta.signature` 和 `citations_delta.citation`，字段缺失会跳过对应输出；已提交引用测试提供对象形状样本。参考是字段消费证据，不能推断它会严格拒绝畸形增量，也不代表真实 Anthropic 会发这类帧。

目标 `AnthropicSseInspector` 旧实现只核对 delta 名称与已知内容块类别，文本、引用、思考、签名增量缺必需字段或类型错误仍能跟随 `message_delta/message_stop` 被记作成功。八例组成的定向测试在旧实现上先红（首例缺 `text`），修复后八例均通过。现已知文本/思考/签名字段必须是字符串，引用必须是对象；空字符串和未知未来内容块类型沿用原有兼容，工具 `partial_json` 校验不变。Claude OAuth HTTP 回环确认同批完整终态中的缺 `text` 增量在成功提交前返回 502，且不记 completed usage。本项不改出站 Messages/count_tokens、Account/Provider/Share 绑定、CL-N7/CL-N8、静态模型目录、receipt schema 或 entitlement 判断。离线反例不能替代真实厂商验收；五份 operation receipt、续线程/beta 接受性及当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Codex。

验证收口：八例畸形字段测试集合先红（首例缺 `text`）后绿，Anthropic inspector 定向 29/29，Claude OAuth HTTP 回环 1/1。完整 Rust 独立串行回归为 library 3400 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo fmt --check`（格式化后）、`cargo check -q`、`cargo clippy --all-targets -- -D warnings`、Claude 私有 receipt Node 252/252、冻结 reference-delta `--check-sources`、离线 wire profile、Provider coverage/Phase 0/UI matrix 合同审计、文档索引和差分空白检查通过。本地服务 smoke 首次因 `cargo run` 编译超过脚本 30 秒启动窗口而未连接，预构建后重跑通过；没有做部署测试。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过其内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。完整 `scripts/static-checks.sh` 会运行 Web UI 测试，本轮遵守禁止 UI 自动化测试要求未运行。未提交，不追加 committed-target observation。

### 18.193 Codex 空文本启动公告的隐藏业务字段提交门槛

2026-10-10，按循环只处理 Codex。`Codex/AGENTS.md` 推荐的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 已提交 HEAD 与工作树均干净。官方 OpenAI Docs 的 Responses 类型参考确认 `output_text.annotations` 字段；CLIProxyAPI 的闭集启动缓冲只检查空文本，其转换器产出空 `annotations`/`logprobs` 数组。这些资料不证明私有订阅账号返回非空引用或 logprobs，也不授权改变出站 wire。

目标 CX-N1 已要求公告必须语义为空，但 `is_empty_startup_part` 旧实现只看空 `text`。非空引用、logprobs 或未知内容字段仍可伪装成 Lifecycle，在客户端已有可见业务数据时保持透明恢复窗口。顶层 part 和 message content 的固定反例在旧实现上先红（首例非空引用）。现已知文本 part 仅在 `text` 为空、`annotations`/`logprobs` 缺失或为空数组且无其他未知字段时算 Lifecycle；refusal part 仅允许空拒绝文本和类型字段。其余 fail closed 为 Business，空数组正例保留。JSON/SSE、WebSocket 与 WS→HTTP fallback 共用分类器，事件正文不改写。只修本地 pre-commit 判定，不改 Codex OAuth/Images 出站、Account/Provider/Share、同账号预算、Images/WS prewarm 开关或 receipt schema。五份真实 operation receipt、厂商接受性及当前未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一单类别为 Cursor。

验证收口：非空引用反例在旧实现上先红，修复后引用、logprobs、未知字段 × 顶层/nested 两路均提交为 Business；空数组与空 summary 保持 Lifecycle。Responses 语义定向 29/29，WebSocket 分类定向 1/1。完整 Rust 独立串行回归为 library 3401 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo fmt --check`、`cargo check -q`、`cargo clippy --all-targets -- -D warnings`、Codex 私有 receipt Node 205/205、冻结 reference-delta `--check-sources`、Provider coverage/Phase 0/UI matrix 合同审计、文档索引、预构建后的本地服务 smoke 与差分空白检查通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。完整 `scripts/static-checks.sh` 会运行 Web UI 测试，本轮按禁令未运行。未提交，不追加 committed-target observation。

### 18.194 Cursor 冷续接去重栅栏在容量满时原子拒绝

2026-10-10，按循环只处理 Cursor。只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 session manager：参考提供 5 分钟 TTL、100 会话容量和 close cleanup，但不定义目标 Server 的跨请求冷续接 claim 语义。其 22 项未提交 Codex 改动排除，`CUR-N2` 的 no-new-wire 结论不变。

目标 `claim_continuation_ids` 旧实现容量满时驱逐未过期的最老已消费 ID；上限 64 下“先消费一个 ID，再提交 64 个竞争 ID”的固定反例先红，原 ID 的重复续接栅栏消失。现整批 ID 在同一写锁内去重并预检，容量不足则完全不插入、不驱逐未过期 claim；调用方在打开新 AgentService run 前显式关闭已获取 parked owner，返回可重试 `503 cursor_continuation_claim_capacity_exhausted`。这只修本地 replay-fence 容量，不改 OAuth/API-key 认证、Account/Provider/Share 固定绑定、正常 h2 续接、protobuf、模型目录、receipt schema v2/revision 2 或 `CUR-N2` 无新 wire 决策。离线压力 fixture 不证明 Cursor 真实接受性；OAuth/API-key 两份真实 receipt 与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Grok。

验证收口：容量 64 的反例先红后绿，Cursor session 定向 24/24；完整 Rust 独立串行回归为 library 3402 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Clippy 首轮只提示新增测试可用 `std::slice::from_ref` 替代单项 clone，等价调整后定向测试 1/1 与 `cargo clippy --all-targets -- -D warnings` 通过；全量 Rust 回归在该测试写法调整前完成，生产代码未再变化。Cursor 私有 receipt Node 95/95、全量后端 Node audit、`cargo check -q`、`cargo fmt --check`、Cursor reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix 合同审计、文档索引、生成 coverage 核对、预构建后的本地服务 smoke 与差分空白检查通过。OAuth/API-key 两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。完整 `scripts/static-checks.sh` 会运行 Web UI 测试，本轮按禁令未运行。未提交，不追加 committed-target observation。

### 18.195 Grok SSE 搜索能力证据等待完整成功流

2026-10-10，按循环只处理 Grok。只读复核 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 Responses 终态扫描器，以及 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 ping 过滤器；后者六项未提交 Codex 内容排除。参考区分成功/失败终态但没有本 Server 多租户搜索 capability evidence 的持久化合同。本项不推断新的 xAI wire，GR-N4 同账号版本/header 接受性仍独立 `live_pending`。

目标 Grok Responses SSE 旁路原在看见 completed `response.output_item.done` 的当个 chunk 就把搜索能力写入 Account；若稍后 `response.failed`，主流失败却已留下搜索能力证据。固定 HTTP 回环反例在旧实现先红。现只累积布尔候选，直到上游 EOF、主 Responses 语义成功终态及本地转换/容量检查全部通过后，再通过既有 Account generation 门禁持久化；失败、缺终态、中断和本地错误不提交。成功终态回环正例通过。此项只修证据提交时机，不改变推理/媒体/WS 出站、Account/Provider/Share 路由、默认 CLI 版本与 group header、remote compaction disabled、receipt schema 或 GR-N4 决策。离线反例不能证明真实 xAI 搜索权益；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号实测和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Kiro。

验证收口：搜索完成后失败的 HTTP SSE 回环先红后绿，完整成功流正例通过，新增定向 2/2；完整 Rust 独立串行回归为 library 3404 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo check -q`、`cargo fmt --check`、Clippy `--all-targets -D warnings`、Grok 私有 receipt Node 159/159、全量后端 Node audit、冻结 Grok reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix 合同审计、文档索引、预构建后的本地服务 smoke 与差分空白检查通过。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。完整 `scripts/static-checks.sh` 会运行 Web UI 测试，本轮按禁令未运行。未提交，不追加 committed-target observation。

### 18.196 Kiro EventStream 同块多帧不误触待解码容量上限

2026-10-10，按循环只处理 Kiro。只读复核 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 decoder：参考也在逐帧解码前用整块 `feed` 输入检查 buffer 上限，故只借鉴其帧/缓冲有界信号，不照搬整块 admission。目标自身已经有严格 CRC、16 MiB 单帧/待解码上限与 `endEvent` 成功合同，本项不推断新的 Kiro/Amazon Q wire。

目标 `EventStreamDecoder::feed` 原把上次剩余字节与本次整块网络输入一次相加比较上限；即使本块是多个合法小帧，也会先报 `BufferOverflow`。32 字节缩小容量下单块三个 16 字节有效帧的固定反例先红，旧实现误报 size 48。现按待解码空间分段接收并随时排出完整帧；同块三帧与不同长度的跨块前缀均通过，真正超限或损坏的未完成帧仍走原失败关闭路径。这只修本地传输分帧，不改变出站请求、EventStream 单帧上限/CRC/终态、请求生命周期预算、Account/Provider/Share 的 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema。离线网络块反例不能证明厂商真实发送方式；八份真实 receipt、KI-N5 专项证据及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Qoder。

验证收口：同块三帧反例在旧实现先红、修复后含 0/8/15 字节跨块前缀均通过；Kiro EventStream wire 定向 11/11。完整 Rust 独立串行回归为 library 3405 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo check -q`、`cargo fmt --check`、Clippy `--all-targets -D warnings`、Kiro 私有 receipt Node 492/492、全量后端 Node audit、冻结 Kiro reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix 合同审计、文档索引、预构建后的本地服务 smoke 与差分空白检查通过。八个 `RUN_REAL=0` auth-kind×region 入口均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。完整 `scripts/static-checks.sh` 会运行 Web UI 测试，本轮按禁令未运行。未提交，不追加 committed-target observation。

### 18.197 Qoder SSE 错误状态不得在 finish 时洗白

2026-10-10，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter；该仓 `AGENTS.md` 要求先调用当前会话不可用的 `project-doc` 技能。读取门禁后未读取其业务源码，也未运行访问外仓源码的差分审计，最新参考增量继续 `unreviewed_skill_gate`。本轮只对照目标 Server 的 Qoder 状态机、既有本地 fixture 与冻结 CLI oracle，不推断新 COSY wire。

目标 `QoderSseDecoder` 和 `QoderChatSseAggregator` 原在接收 `[DONE]` 后遇到后续业务帧时，`push` 虽报错却未记住失败，随后 `finish` 可能错误返回成功。两条固定反例在旧实现先红。现两处 `push` 错误均锁存；decoder 的 `finish` 错误也锁存。第一次错误仍按原上游/本地协议或容量分类返回，之后的 `push`/`finish` 固定失败且不回显上游内容，decoder 已失败时不再报告完成。正常唯一终态与 EOF、COSY endpoint/header/body、Global/CN 站点及 OAuth/PAT rail、Account/Provider/Share 绑定、receipt schema v2/revision 2 与 401 预算均不变。离线反例不能证明真实厂商发送序列；Global OAuth、Global PAT、CN OAuth 三份真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及当前未提交工作树的 committed-target observation 均待完成。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 CodeBuddy。

验证收口：两条反例旧实现先红、新实现 2/2 通过；完整 Rust 独立串行回归为 library 3407 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Qoder 私有 receipt Node 281/281、冻结 CLI oracle 9/9、自包含 reference-delta、全量后端 Node audit 1909/1909、Provider/Phase 0/coverage/UI matrix、文档索引、transform coverage、`cargo check -q`、`cargo fmt --check`、Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 与差分空白检查通过。未运行会访问 TokenRouter 业务源码的 `--check-sources`；完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。Global OAuth、Global PAT、CN OAuth 的三个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.198 CodeBuddy SSE 错误状态不得在 finish 时洗白

2026-10-10，按循环只处理 CodeBuddy。只读复核 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 WorkBuddy SSE 实现：`sse.go::Aggregate` 在首个 `[DONE]` 即停止扫描，不能代替目标 Server 的唯一终态加 EOF 合同；两个未跟踪文档继续排除。冻结 CodeBuddy reference-delta 的 committed-object 审计通过，本轮不推断新厂商 wire。

目标 `CodeBuddySseDecoder` 与 `CodeBuddyChatSseAggregator` 原在 `[DONE]` 后的业务帧被 `push` 拒绝后未记住错误，随后 `finish` 可错误返回成功；两条固定反例在旧实现先红。现两处 `push` 错误均锁存，decoder 的 `finish` 错误也锁存。第一次错误仍保留原上游/本地协议或容量分类，后续 `push`/`finish` 固定失败、不回显上游内容；decoder 失败后不再报告完成。正常唯一 `[DONE]` 加 EOF、Intl/CN 站点、WorkBuddy endpoint/header/body、Account/Provider/Share 绑定、同账号 401 预算、receipt schema v2/revision 2 与 CB-N4 disabled 决策均不变。离线反例不证明真实上游发送该序列；Intl/CN 两份真实 receipt、CB-N4 CN 模型目录/接受性及当前未提交工作树 committed-target observation 均缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别回到 Antigravity。

验证收口：两条 `[DONE]` 后错误反例旧实现先红，新实现通过；另补 decoder 的 `finish` 失败不可由后续数据修复，三条新测试均通过。完整 Rust 独立串行回归为 library 3410 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit 1909/1909、CodeBuddy reference-delta `--check-sources`（仅冻结已提交对象）、Provider/Phase 0/coverage/UI matrix、文档索引、transform coverage、`cargo check -q`、`cargo fmt --check`、Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，本轮按禁令未运行。Intl/CN 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.199 Antigravity replay 保留有签名文本段边界

2026-10-10，按循环只处理 Antigravity。只读复核 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 和 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`；前者的已提交 replay 测试明确要求有签名文本后接无签名文本时不串签名，也分别匹配多个有签名文本段。后者仅提供 Gemini part 签名保真的差分参考，其 turn-anchor 策略不移植。冻结 reference-delta `--check-sources` 已通过；不推断新 Google wire。

目标 `merge_stream_content` 原把同类相邻文本无条件拼接，已签名文本后的无签名文本会被合并成新的 fingerprint，第二个签名也可能覆盖第一个；客户端分段续接历史遂无法精确回填。目标闭环反例在旧实现先红。现只允许未签名的文本前缀继续吸收后续碎片；非空且已有有效签名的文本段立即关闭，后续文本另起一段。签名+无签名、两个不同签名段以及未签名前缀+末段签名的正例均通过。只修 replay 捕获，不改变主数据面成功判定、终态前 replay 提交策略、Antigravity/AGY 出站 wire、Account/Provider/Share 绑定、AG-N6 requestType、AG-D3/compaction disabled 或 receipt schema v2/revision 2。离线分段反例不证明真实上游发送方式；两条真实 OAuth receipt、AG-N6/AG-D3 专项厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Claude。

验证收口：有签名段后接无签名段的目标 replay 闭环先红后绿；不同签名段和未签名前缀+末段签名正例通过，相关 stream replay 定向 4/4。完整 Rust 独立串行回归为 library 3412 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt Node 165/165、全量后端 Node audit 1909/1909、冻结 reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、文档索引、transform coverage、`cargo check -q`、`cargo fmt --check`、Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，本轮按禁令未运行。`antigravity_oauth` 与 `agy_oauth` 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.200 Claude SSE 协议错误不得回显不可信事件名

2026-10-10，按循环只处理 Claude。只读复核 `Claude/AGENTS.md` 推荐、工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `claude_executor_stream.go::validateClaudeStreamingResponse`：其畸形流使用固定本地错误文字，普通上游 `error` 事件则另行处理。参考不定义目标 Server 的精确 SSE 状态机；本轮只补强目标本地诊断的保密边界，不推断新 Anthropic wire。

目标 `AnthropicSseInspector` 原把不匹配的 SSE `event` 名、未知 payload `type` 及未知 `delta.type` 插入协议错误；`forwarder` 将错误作为 502 返回，下游可见上游恶意帧中的 `Bearer` 文本。检查器单测与 HTTP 回环在旧实现先红。现不匹配事件返回固定说明，未知 payload 事件在 `message_start` 前与未知 delta 类型使用固定名称；已知 `text_delta` 等诊断保留。正常上游错误事件、成功流和出站请求不变；Account/Provider/Share 绑定、CL-N7/CL-N8、receipt schema 与已冻结参考差分均不动。离线恶意帧不证明真实 Anthropic 会发送该内容；五份真实 operation receipt、续线程/beta 厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Codex。

验证收口：两条新反例先红后绿；完整 Rust 独立串行回归为 library 3414 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 252/252、全量后端 Node audit 1896/1896、smoke Node 13/13、冻结 Claude reference-delta `--check-sources`、离线 wire profile、Provider/Phase 0/coverage/UI matrix、文档索引、transform coverage、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 会运行 Web UI 测试，按禁令未运行。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。首次本地服务 smoke 因尚未预构建、30 秒启动窗口内仍在编译而超时；预构建后重跑通过。未提交，不追加 committed-target observation。

### 18.201 Codex Responses 检查器失败状态不可洗白

2026-10-10，按循环只处理 Codex。`openai-docs` 技能引导查阅官方 [Streaming API responses](https://developers.openai.com/api/docs/guides/streaming-responses)：Responses 流以带类型的 SSE 语义事件表达，`response.completed` 与 `error` 不可混淆；官方文档不规定本 Server 的检查器复用状态。只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 和 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 已提交 Codex WS 读取器：两者都以明确终态/错误结束当前流读取，但不定义目标检查器的失败锁存。本项只加固目标本地状态，不推断私有 Codex wire。

目标 `ResponsesSseInspector` 原在已有成功终态后，下一次 `push` 即使因冲突终态或畸形 JSON 报错，`terminal()` 仍报告成功，随后 `finish` 也可能成功；`finish` 因缺终态报错后，迟到完成事件同样可能修复同一检查器。两组固定反例先红。现首次 `push`/`finish` 错误仍保留原协议、容量或缺终态分类；后续 `push`/`finish` 固定失败，且不再暴露成功终态。正常成功/失败/不完整流和唯一可选 `[DONE]` 不变；HTTP 转发器此前已在首次错误时中断，本项不声称改变已提交的下游响应。Codex OAuth/Images/WS 出站、Account/Provider/Share 绑定、同账号预算、预热开关和 receipt schema 不变。离线状态机反例不证明真实厂商行为；Astra、三个 Image 2.5 变体、WS prewarm 五份真实 receipt、厂商接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Cursor。

验证收口：两组新反例先红后绿；首次完整 Rust 串行回归仅账号 Cursor Dashboard quota 并发测试因外部响应状态从预期 400 变成 502 而失败，该测试单独重跑通过，后续无并行 Node 审计的完整重跑为 library 3416 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205、全量后端 Node audit 1896/1896、smoke Node 13/13、冻结 Codex reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、文档索引、transform coverage、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 和差分空白检查通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。未提交，不追加 committed-target observation。

### 18.202 Cursor 冷续接已消费 ID 不得重复 claim

2026-10-10，按循环只处理 Cursor。只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 session manager：参考保留同一 h2 流、5 分钟 TTL、100 会话容量及 close cleanup，但没有目标 Server 的跨请求冷续接 claim；其未提交 Codex 文件继续排除。`CUR-N2` 仍为 `reviewed_no_wire_delta`，本项不推断新的 Cursor wire。

目标 `claim_continuation_ids` 旧实现只计数未占用 ID，不拒绝同作用域已消费 ID；重用旧 ID 会刷新其短期栅栏并写入同批新 ID。另一条 parked 会话复用旧 tool-call ID 时，agent driver 因而能返回可用冷续接快照。session manager 的“旧 ID + 新 ID”整批原子反例和第二 parked owner 的闭环反例均先红。现持有同一写锁时先检查全部精确作用域 ID，任一未过期旧 claim 即拒绝且不写新 ID；调用方关闭已获取的 parked owner，并用现有 `409 cursor_continuation_in_progress` 回报。不同 scope 的同字面 ID、正常新续接、容量耗尽时独立可重试的 503 保持。OAuth/API-key 认证、Account/Provider/Share 固定绑定、正常 h2 续接、protobuf、模型目录和 receipt schema v2/revision 2 不变。离线重复 ID 不能证明厂商实际发送此形状；OAuth/API-key 两份真实 receipt 与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Grok。

验证收口：两条新反例先红后绿。完整 Rust 独立串行回归在一处新增测试的等价 Clippy 写法调整前为 library 3418 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）；调整后两条定向测试均通过。Clippy 首轮仅提示测试单项 clone 应改为 `std::slice::from_ref`，等价修改后 `cargo check -q`、rustfmt 与严格 Clippy 均通过。Cursor 私有 receipt Node 95/95、全量后端 Node audit 1896/1896、smoke Node 13/13、冻结 Cursor reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、文档索引、transform coverage、预构建后的本地服务 smoke 和差分空白检查通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。OAuth/API-key 两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。未提交，不追加 committed-target observation。

### 18.203 Grok SSE 成功终态之后继续读到 EOF，延迟提交质量证据

2026-10-10，按循环只处理 Grok。只读复核 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `quality_retry_scan.go` 和 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 ping 过滤器；后者六项未提交 Codex 文件继续排除。参考只提供终态分类和兼容信号，不定义本 Server 的 EOF 与质量指标提交合同，本项不推断新 xAI wire。

目标流转发器原在主 Responses 语义或终态检测器看到 `response.completed` 后就停止读取上游，把语义完成误当作传输 EOF；后续跨块 `response.failed` 根本进不了严格检查器，§18.195 搜索证据虽推迟到所谓 EOF，仍可能提前持久化。Grok 质量指标更早，在完成帧所在 chunk 便立即入账。跨块“先完成搜索与响应、再给冲突失败终态”的 HTTP 回环在旧实现先红：下游只看到完成帧。现仅 Grok Responses SSE 在成功终态后继续读到真实 EOF，尾随冲突由主语义检查器拒绝；搜索能力与脱敏质量样本都等 EOF、语义成功及尾部转换/容量检查通过后再提交。失败流两者均不提交，完整成功流两者均提交。HTTP JSON/原生 WS 的质量路径、其他 Provider 的终态读取策略、Grok 出站 wire、Account/Provider/Share 绑定、GR-N4、remote compaction disabled 与 receipt schema 均不变。离线尾随帧不是厂商真实发送证据；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号版本/header 接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 Kiro。

验证收口：跨块失败和完整成功的 HTTP 回环先红后绿，新增定向 1/1；完整 Rust 独立串行回归为 library 3419 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo check -q`、`cargo fmt --check`、Clippy `--all-targets -D warnings`、Grok 私有 receipt Node 159/159、全量后端 Node audit 1896/1896、smoke Node 13/13、冻结 Grok reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix 合同审计、文档索引、transform coverage、预构建后的本地服务 smoke 与差分空白检查通过。三个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。本轮未提交，不追加 committed-target observation。

### 18.204 Kiro EventStream 流式成功终态必须等待真实 EOF

2026-10-10，按循环只处理 Kiro。只读复核 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/anthropic/handlers.rs::create_sse_stream`：参考持续读取上游 body，并把读取错误与 EOF 分开；它不定义本 Server 的 `endEvent` 成功合同。本项仍以目标冻结的 `endEvent_required`、`framesAfterTerminal=error` 合同为准，不推断新 Kiro/Amazon Q wire。

目标非流式转换已扫描整份 EventStream body，能拒绝 `endEvent` 后的帧；流式转换却在当前 chunk 看到 `endEvent` 就调用 decoder `finish` 并停止读取，上游下一块的完整业务帧或半截帧被忽略，错误输出正常 Anthropic `message_stop`。两种跨块反例在旧实现先红。现流式桥接器在成功终态后继续读到真正 EOF：后续完整帧由终态栅栏拒绝，半截帧由 EOF 检查拒绝；只有 `endEvent` 后正常 EOF 的正例输出 `message_stop`。尾随传输/idle 错误也不再被提前成功掩盖。请求构造、EventStream CRC/容量、Account/Provider/Share 与 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 和 receipt schema v1/revision 1 不变。离线尾随帧不是厂商真实发送证据；八份真实 receipt、KI-N5 独立计量证据及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 Qoder，其推荐 TokenRouter 业务源码仍需遵守 `project-doc` 技能门禁。

验证收口：同一新增测试覆盖跨块完整帧、截断帧和正常 EOF，先红后绿，定向 1/1；完整 Rust 独立串行回归为 library 3420 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo check -q`、`cargo fmt --check`、Clippy `--all-targets -D warnings`、Kiro 私有 receipt Node 492/492、全量后端 Node audit、smoke Node 13/13、冻结 Kiro reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix 合同审计、文档索引、transform coverage、预构建后的本地服务 smoke 和差分空白检查通过。八个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。本轮未提交，不追加 committed-target observation。

### 18.205 Qoder SSE wrapper 显式形状不得降级为空心跳

2026-10-10，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter；其 `AGENTS.md` 要求先调用当前会话未加载的 `project-doc` 技能，故读取门禁后即停止，没有读取业务源码，也未运行访问外仓的 `--check-sources`。最新参考业务增量继续为 `unreviewed_skill_gate`。本项只依据目标 Qoder decoder、已有空心跳兼容规则和冻结 CLI oracle，不推断新 COSY wire。

目标 `QoderSseDecoder::decode_event` 原把 `null`、数组和标量 wrapper，以及显式非字符串 `body`，都通过 `get(...).and_then(...).unwrap_or("")` 降级为空心跳。随后 `[DONE]` 可以令整条流假成功。六种畸形形状的单元反例在旧实现先红；现要求 wrapper 为 JSON 对象、显式 `body` 必须为字符串，缺失 `body` 的 `{}` 空心跳仍兼容，畸形形状返回固定本地协议错误并锁存失败。Global OAuth、CN OAuth、Global PAT × Claude/Codex/Gemini 九组 HTTP 回环均返回 502、只向绑定账号发一次生成请求且不触发 refresh。正常 wrapper、唯一终态+EOF、COSY endpoint/header/body、Share/Account/Provider 绑定、共享 attempt budget 与 receipt schema v2/revision 2 不变。离线形状反例不证明厂商真实发送；三条 rail 真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 CodeBuddy。

验证收口：两条新增定向测试各 1/1，完整 Rust 独立串行回归为 library 3422 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo check -q`、`cargo fmt --check`、Clippy `--all-targets -D warnings`、Qoder 私有 receipt Node 281/281、冻结 CLI oracle 9/9、自包含 reference-delta、全量后端 Node audit、smoke Node 13/13、Provider/Phase 0/coverage/UI matrix 合同审计、文档索引、transform coverage、预构建后的本地服务 smoke 与差分空白检查通过。没有运行访问 TokenRouter 业务源码的 `--check-sources`；完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。Global OAuth、Global PAT、CN OAuth 三个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.206 CodeBuddy choice 形状与 index 的流/非流一致性

2026-10-10，按循环只处理 CodeBuddy。`CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 WorkBuddy 聚合器只消费首个 choice，宽松流复制器未提供目标 Server 的多 Surface choice 校验合同；两个未跟踪文档继续排除。本项只修目标自身的协议完整性，不推断新厂商 wire。

目标非流聚合器原拒绝非对象 choice、非零 index，却把显式不可解析 index 默认为 0；流式 decoder 则可转发非对象、非零和不可解析 index，再以 `[DONE]`+EOF 误记成功。固定形状/index 反例在旧实现先红。现两路共用校验：choice 必须是对象，显式 index 必须可解析为 0；缺省 index、数值 0 与兼容字符串 `"0"` 保持。Intl/CN × Claude/Codex/Gemini × 流/非流的十二组 HTTP 回环均在成功提交前返回 502，只向绑定账号发一次生成请求且不触发 refresh。正常唯一终态+EOF、WorkBuddy endpoint/header/body、Share/Account/Provider 绑定、同账号 401 预算、receipt schema v2/revision 2 与 CB-N4 disabled 决策不变。离线反例不能证明厂商发送该形状；两站真实 receipt、CN 模型目录/接受性和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别回到 Antigravity。

验证收口：畸形 choice 单测先红后绿，十二组 Intl/CN HTTP 回环通过；完整 Rust 独立串行回归为 library 3424 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo check -q`、`cargo fmt --check`、严格 Clippy、本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。Intl/CN 两项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.207 Antigravity replay 历史指纹保留工具业务字段

2026-10-10，按循环只处理 Antigravity。只读复核 `Antigravity/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` reasoning replay context fingerprint 用例，及 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` Gemini part 签名位置处理；参考不定义目标 Server 的哈希算法。本项不推断新 Google wire。

目标 `canonical_history_value` 原递归忽略 `thoughtSignature`、`thought_signature`、`cache_control`，使工具参数或工具结果里同名业务键变化时仍匹配旧历史指纹，并可能把旧签名回填至新上下文。两条闭环反例在旧实现先红。现只在 Gemini content/part 元数据位置剔除这些字段，并处理 part 的 `extra_content.google.thought_signature`；工具参数/结果在任意嵌套层级完整参加指纹和无 ID 调用参数匹配。原有签名/cache metadata 差异仍允许 replay，真实业务差异拒绝。只修 AG-D3 本地上下文隔离，不改 Antigravity/AGY 出站 wire、Account/Provider/Share 绑定、AG-N6 requestType、终态前 replay 提交、compaction disabled 决策或 receipt schema v2/revision 2。离线反例不证明厂商发送同名业务键；两条真实 OAuth rail receipt、AG-N6/AG-D3 专项接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一类别为 Claude。

验证收口：两条闭环反例旧实现先红，最终 replay 定向 22/22；补充验证真正的 part 签名/cache 元数据及 `extra_content.google.thought_signature` 仍不改变历史匹配。最终代码完整 Rust 独立串行回归为 library 3426 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt Node 165/165、全量后端 Node audit 1909/1909、冻结 reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo check -q`、`cargo fmt --check`、严格 Clippy、本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。`antigravity_oauth` 与 `agy_oauth` 两项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.208 Claude SSE 检查器失败状态不可洗白

2026-10-10，按循环只处理 Claude。只读复核 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `claude_executor_stream.go::validateClaudeStreamingResponse`：参考把畸形流和缺必要事件视为失败，不规定目标检查器对象的复用状态。本项只修目标本地语义生命周期，不推断新 Anthropic wire。

目标 `AnthropicSseInspector` 原在成功 `message_stop` 后遇到冲突帧时，`push` 报错但 `terminal()` 仍为成功且 `finish` 仍可通过；`finish` 因缺终态失败后，迟到 delta/stop 也可补成成功。两组固定序列在旧实现先红。现首次 `push`/`finish` 错误保留原分类并锁存失败；其后调用固定返回脱敏本地错误，成功终态与 message ID 不再暴露。正常 start/delta/stop、上游 error、流式/非流式响应和出站 Claude OAuth Messages/count_tokens 不变。HTTP 转发器此前已在首次错误时中断，本项不声称改变已提交的下游响应；Account/Provider/Share、CL-N7/CL-N8、静态模型目录、receipt schema 与 entitlement 决策不变。离线反例不证明厂商真实发送该序列；五份真实 operation receipt、thread/beta 接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一类别为 Codex。

首次全量 Rust library 3428 passed / 1 ignored，但集成 `claude_share_never_fails_over` 偶发观察到一次活端点请求；该测试单独重跑通过。检查夹具发现两个 Claude no-failover 测试先释放死端口、后绑定活端口，允许 OS 将同一临时端口立即复用。现令两监听在选端口时同时存活，确认地址互斥后才释放死端口；两条定向集成测试通过。这是测试夹具的确定性修复，不改变生产路由。完整 Rust 独立重跑与最终静态检查见后续验证收口。

验证收口：两条检查器状态反例先红后绿，Anthropic 语义定向 19/19；修正夹具后最终代码的完整 Rust 独立串行回归为 library 3428 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Clippy 首轮仅提示两个新增 getter 的 `bool::then` 闭包可简化；等价修改后 `cargo check -q`、`cargo fmt --check`、严格 Clippy 和完整 Rust 重跑均通过。Claude 私有 receipt Node 252/252、全量后端 Node audit 1909/1909、冻结 reference-delta `--check-sources`、离线 wire profile、Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.209 Codex 空输出项公告的字段闭集门槛

2026-10-10，按循环只处理 Codex。官方 [OpenAI Docs Responses streaming 指南](https://developers.openai.com/api/docs/guides/streaming-responses) 确认流由带类型语义事件构成，不定义私有订阅的未知字段形状。`Codex/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 启动缓冲仅接受已知空输出项；干净 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 没有目标 Server 的字段闭集语义。本项只借鉴“空公告先确认”的边界，不推断新 Codex wire。

目标 `is_empty_startup_output_item` 原仅查已知内容字段是否为空，message、reasoning、function_call、custom_tool_call item 上附带未知业务字段时仍归为 Lifecycle；对象型 role 等伪元数据也可钻空。五种固定反例的首例先在旧实现上复现错误分类，现对四类 item 的已知字段分别设闭集，身份/状态/名称元数据须为字符串；未知字段与异常元数据一律 Business。正常 `id/status/role/call_id/name` 空公告保持 Lifecycle，HTTP JSON/SSE 与 WebSocket 共用分类器。本项只修 CX-N1 预提交门槛，不改变事件正文、出站 Responses/Images/WS、Account/Provider/Share、同账号恢复预算、预热启用状态或 receipt schema。离线反例不能证明厂商发送该形状；五份真实 operation receipt、厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一类别为 Cursor。

验证收口：未知 item 字段首例在旧实现上先红，修复后五种反例均为 Business，带标准 `status/call_id` 的空公告仍为 Lifecycle；Codex 语义定向 32/32。完整 Rust 独立串行回归为 library 3429 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、冻结 Codex reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、差分空白检查通过；生成的 `docs/provider/coverage.md` 已核对，无需改动。本地服务 smoke 首次因二进制未预构建而超过 30 秒启动等待，预构建后重跑通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；未做部署测试。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.210 Cursor 过期索引候选不得遮蔽有效停放会话

2026-10-10，按循环只处理 Cursor。只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 session manager：参考在按工具 ID 查找及按 conversation 获取之前清理过期会话；未提交 Codex 工作树内容排除。参考不定义目标 Server 的 scope-fenced response/tool 二级索引，本项不推断新 Cursor protobuf 或厂商 wire，`CUR-N2` 的 no-new-wire 结论不变。

目标 `resolve_index_binding` 原把仍留在主表的过期停放会话计入候选。同一作用域下过期和有效会话共用响应 ID 与工具 ID 时，两种查找都被误判为歧义；固定反例的首个响应 ID 断言在旧实现先红。现索引查找先执行惰性 TTL 清理，读取候选时再次排除已关闭/已过期项；持锁候选仍保守计入，防止误选。过期 owner 关闭并清理其索引，有效 owner 可以被解析；真正的两个有效候选仍拒绝归属。只改进本地会话查找，不改 AgentService h2/protobuf、OAuth/API Key、Account/Provider/Share 单绑定、冷续接 claim、完成响应缓存、模型目录或 receipt schema v2/revision 2。离线反例不证明 Cursor 真实 ID 行为；OAuth/API Key 两份真实 receipt、厂商接受性和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一类别为 Grok。

验证收口：同 scoped ID 的过期/有效双候选反例先红后绿，两个有效候选仍由既有测试拒绝。完整 Rust 独立串行回归为 library 3430 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、冻结 Cursor reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 和差分空白检查通过；生成的 `docs/provider/coverage.md` 已核对，无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。OAuth/API Key 两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.211 Grok replay 工具名与调用 ID 不得隐式裁剪

2026-10-10，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 与 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交实现；后者六处未提交 Codex 工作树改动排除。参考的 reasoning 隔离、Responses 终态扫描与 SSE ping 兼容只作为差分信号，不定义本 Server 的工具身份规范化规则。本项按目标既有精确上下文合同收紧 replay，不推断新 xAI wire。

目标 `call_fingerprint` 原对工具名 `trim()` 后哈希，使客户端把 `lookup` 改成 ` lookup ` 时错误复用旧 reasoning proof；`capture_output` 与 `apply` 同样裁剪 `call_id`，使上游捕获与客户端预检失去原始 ID 边界。固定闭环反例在旧实现先红。现工具名只检查非空白及 256 字节上限，原始字符串参加指纹；调用 ID 在上游捕获和客户端预检都要求非空、至多 256 字节且无首尾空白。客户端改名、带空白 ID、上游带空白 ID 或上游带空白名称后客户端改名均失败关闭；上游和客户端完全一致的合法带空白工具名仍可命中。只改变本地 Grok reasoning replay 判定，不改发往 xAI 的字段、HTTP/SSE/WS 生命周期、Account/Provider/Share scope、一次明确拒绝恢复预算、GR-N4 或 receipt schema。离线反例不证明真实厂商发送这些形状；inference/media/remote_compaction 三项真实 receipt、GR-N4 同账号版本/header 接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 Kiro。

验证收口：固定身份反例旧实现先红，新实现定向 1/1；完整 Rust 独立串行回归为 library 3431 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit 1896/1896、smoke Node 13/13、冻结 Grok reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、本地服务 smoke 与差分空白检查通过；生成的 `docs/provider/coverage.md` 已核对，无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.212 Kiro 工具分片身份不得漂移或重用

2026-10-10，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/anthropic/stream.rs::ToolJsonAccumulator` 与 `src/kiro/model/events/tool_use.rs`：参考确认 `toolUseEvent` 按 ID 汇集任意 JSON 分片、`stop=true` 后解析，并允许无参工具收尾；它没有给出目标 Server 的同 ID 名称漂移或完成后重用合同。本项按目标多 Surface 工具身份边界收紧处理，不推断新的 Kiro/Amazon Q wire。

目标 `ToolJsonAccumulator` 原忽略同一 `toolUseId` 后续分片中不同的非空工具名，仍拼接参数并以首个名称输出；一次调用完成后再收到同 ID 也可能输出第二次工具调用。固定闭环反例在旧实现先红：`Read` 首片与 `Write` 末片被错误合成一次 `Read` 调用。现两种冲突都固定返回脱敏的 `KIRO_EVENT_STREAM_INVALID`；非流式转换与真实流桥接路径均拒绝，流式失败后不输出正常 `message_stop`。同名分片、首片空名后补名、无参工具恢复及已停止但 JSON 无效时的既有 best-effort 边界保留。已完成 ID 集合纳入流生命周期内存核算。只改本地工具事件一致性，不改 EventStream CRC/终态/EOF、出站请求、Account/Provider/Share 与 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线冲突帧不证明真实厂商发送；八份真实 receipt、KI-N5 独立计量证据及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 Qoder，其推荐 TokenRouter 业务源码仍需遵守 `project-doc` 技能门禁。

验证收口：身份冲突反例旧实现先红，修复后定向 1/1；完整 Rust 独立串行回归为 library 3432 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Kiro 私有 receipt Node、全量后端 Node audit 与 smoke Node 13/13、冻结 Kiro reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成的 `docs/provider/coverage.md` 已核对，无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。八个 auth-kind×region `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.213 Qoder 工具调用显式 index 必须保留身份边界

2026-10-10，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter，但当前 HEAD `6d676f1d100e11f2f068949f5779e960c862079c` 的 `AGENTS.md` 要求先使用本会话未加载的 `project-doc` 技能；读取门禁后即停止，未读取其业务源码或运行 `--check-sources`。最新业务增量继续 `unreviewed_skill_gate`。本项仅依据目标 Qoder codec/聚合器、既有单选项与 128 工具上限合同，以及冻结的自包含 CLI oracle，不推断新的 COSY wire。

目标 `canonical_qoder_message_delta` 原把显式非法 `tool_calls[].index` 回退为数组位置，`QoderChatSseAggregator` 回退为 0，兼容工具事件外层 `index` 也回退为 0；`index: null` 因而可被改写为有效工具分片，之后用 `[DONE]` 结束。固定反例在旧实现先红。现三处共用校验：缺省继续按原位置/0，显式值须可解析为非负整数且小于 128；`null`、非法字符串、负数和越界值在转发或聚合前返回本地 502，数字字符串等既有合法写法保持。Global OAuth、CN OAuth、Global PAT × Claude/Codex/Gemini 九组 HTTP 回环均失败关闭，只向绑定账号发一次生成请求且不触发 refresh。仅修本地工具身份，不改 COSY 签名/endpoint/body、site/rail 选择、Account/Provider/Share 单绑定、唯一终态+EOF、共享 attempt/401 预算或 receipt schema v2/revision 2。离线畸形索引不证明厂商发送；三条 rail 真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 CodeBuddy。

验证收口：显式 `index: null` 首例旧实现先红，修复后定向 1/1，九组 HTTP 回环通过。完整 Rust 最终独立串行回归为 library 3434 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。首轮在并行 Node 审计负载下，独立 `state::tests::accepted_provider_commit_survives_caller_cancellation` 偶发失败；该测试仅固定等待 20 ms 后取消调用，单独复跑及无并行负载的最终全量回归均通过，未把首轮失败隐瞒为通过。Qoder 私有 receipt Node、全量后端 Node audit 与 smoke Node 13/13、自包含 Qoder reference-delta、冻结 CLI oracle、Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成的 `docs/provider/coverage.md` 已核对，无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。Global OAuth、Global PAT、CN OAuth 的三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.214 CodeBuddy 工具调用显式 index 不得降级为缺省

2026-10-10，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 `internal/providers/workbuddy/sse.go::mergeToolCalls`：参考按 index 聚合工具分片，但其宽松解码不定义目标 Server 的严格成功门槛；两个未跟踪文档排除。目标 `CodeBuddyChatSseAggregator` 原在显式 `index` 不可解析或为负数时回退到 ID/位置/空位，流式 decoder 则未检查工具 index，因此畸形工具分片可能被 `[DONE]` 加 EOF 洗成成功。固定 `index: null` 反例在旧实现先红。

现两路径共用索引校验：显式值必须能解析为 0～127 的非负整数；`null`、非法字符串、负数和 128 越界值在转发或聚合前失败。缺省 index 保留现有 ID/位置回退，合法数字字符串继续兼容。Intl/CN × Claude/Codex/Gemini × 流/非流十二组本地 HTTP 回环均拒绝畸形值，只向绑定账号发送一次生成请求且不触发 refresh。仅修本地工具分片身份，不改 WorkBuddy endpoint/header/body、Account/Provider/Share 绑定、唯一终态+EOF、receipt schema v2/revision 2 与 CB-N4 disabled 决策。离线反例不能证明厂商真实发送；两站真实 receipt、CB-N4 CN 模型目录/接受性及当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别回到 Antigravity。

验证收口：显式 `index: null` 旧实现先红，修复后定向 5/5（含十二组双站三 Surface 流/非流 HTTP 回环）；完整 Rust 独立串行回归为 library 3437 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit/smoke 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy 与差分空白检查均通过；生成的 `docs/provider/coverage.md` 已核对，无需改动。本地服务 smoke 首次因脚本默认等待 30 秒而编译缓存不一致、服务尚未启动，改用已预构建的 `CARGO_INCREMENTAL=0` 环境后完整通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。Intl/CN 两个 `RUN_REAL=0` 入口仍各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.215 Antigravity replay 同 ID 工具调用必须再核对语义

2026-10-10，按循环只处理 Antigravity。只读核对推荐且干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `TestPrepareAntigravityGeminiReasoningReplayRejectsReusedIDWithChangedCall` 和对应工具查找实现：同一 ID 复用但名称/参数已改时，参考拒绝旧签名；另一推荐的干净 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 思考签名存取只作差分背景，不定义目标算法。本项按目标 AG-D3 上下文隔离收紧实现，不推断新 Google wire。

目标 `patch_model_content` 原只要命中缓存 ID，就把旧 `thoughtSignature` 注入当前调用，哪怕工具名或参数已变；旁边再放一条与旧调用语义相同的新 ID 调用也不能抵消这一错误。固定“同 ID 改参数”和“同 ID 改名”反例在旧实现先红。现同 ID 候选必须唯一且 name/规范化 args 精确匹配，否则整轮 replay 不应用，不继续退到其他语义候选；无 ID 的既有语义/出现序号匹配不变。Antigravity OAuth 与 AGY OAuth 的 Gemini HTTP 回环确认改后参数原样出站、旧签名不再注入、续接只到当前绑定账号一次。只改本地签名关联，不改 Antigravity/AGY endpoint、请求元数据、Account/Provider/Share scope、AG-N6 requestType、终态提交、compaction disabled 或 receipt schema v2/revision 2。离线 fixture 不证明 Google 实际发送这种历史；两条真实 rail receipt、AG-N6/AG-D3 专项接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Claude。

验证收口：同 ID 改参单元反例在旧实现先红，修复后与双 rail HTTP 回环均通过；完整 Rust 独立串行回归为 library 3439 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt Node 165/165、全量后端 Node audit/smoke 1909/1909、冻结 Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成的 `docs/provider/coverage.md` 核对同步，无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。两条 `RUN_REAL=0` rail 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.216 Claude 终止原因后的业务内容不得伪装为成功

2026-10-10，按循环只处理 Claude。只读复核 `Claude/AGENTS.md` 推荐且干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `claude_executor_stream.go::validateClaudeStreamingResponse`：参考仅要求 `message_start` 与 `message_delta` 并检查 JSON 基本形状，不定义本 Server 的后终止业务栅栏。本项依据目标 Anthropic SSE/JSON 语义守卫收紧，不从参考推断新厂商 wire。

目标旧实现把显式布尔、数字或空白 `message_delta.delta.stop_reason` 当作合法进度；`end_turn` 已出现后仍可收新的内容块，最后 `message_stop` 被记为成功。固定非法类型和晚到内容反例先红。现 Messages JSON 与 SSE 共用终止字段类型校验：`stop_reason` 缺省/`null` 或非空字符串，`stop_sequence` 缺省/`null` 或字符串；SSE 非空原因后拒绝新的内容块与冲突的第二原因。没有原因的 usage-only delta 后继续内容仍兼容，同名原因及独立后续 usage 保留。Claude OAuth HTTP 回环确认迟到块在成功提交前返回 502，不写 completed 用量。仅改本地成功判断，不改出站 Messages/count_tokens、Account/Provider/Share 绑定、CL-N7/CL-N8、静态模型目录或 receipt schema v1/revision 1。离线反例不证明厂商发送该顺序；五份 operation 真实 receipt、thread/beta 接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Codex。

验证收口：非法 `stop_reason=false` 首例旧实现先红；修复后 Anthropic 语义单测 21/21、Claude OAuth 晚到内容 HTTP 回环通过。完整 Rust 独立串行回归为 library 3442 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 252/252、全量后端 Node audit/smoke 1909/1909、冻结 Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成的 `docs/provider/coverage.md` 核对同步，无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.217 Codex 错误帧优先于后续完成终态

2026-10-10，按循环只处理 Codex。只读核对 `Codex/AGENTS.md` 推荐且干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 和 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`：两者将 Codex 上游 `error` 视为失败边界，但没有目标 Server 的 HTTP SSE 双路径终态优先级合同。本轮只依据参考的失败信号与目标既有 `error`/`response.completed` 分类收紧本地实现，不推断新私有 wire。

目标 `ResponsesSseInspector` 原暂存 `error`，随后 `response.completed` 会被记为成功；`ResponsesSseAggregator` 也保留 `last_error` 却返回 completed JSON。跨网络块两条固定反例均先红。现流式检查器将先到错误提升为失败终态，不向下游回显伪完成帧；非流聚合器以原错误消息报上游失败。原有 `error` 后 EOF、`[DONE]` 或显式 `response.failed` 的失败路径不变。仅修普通 Codex HTTP Responses SSE/非流聚合，不改变 WebSocket/Images 出站、Account/Provider/Share 单绑定、同账号恢复预算或 receipt schema。离线冲突反例不能证明厂商真实发送；五份 operation 真实 receipt、厂商接受性和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 Cursor。

验证收口：两条 `success_after_error_frame` 回归先红后绿；完整 Rust 独立串行回归为 library 3444 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205、全量后端 Node audit/smoke、冻结 Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 均通过；生成的 `docs/provider/coverage.md` 已核对无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.218 Cursor 停放会话须在无后续请求时主动过期

2026-10-10，按循环只处理 Cursor。只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 session manager：其停放 h2 会话有可取消的 idle timer，获取或关闭会话时取消；22 项未提交 Codex 工作树内容排除。参考只提供主动资源清理的差分证据，不定义本 Server 的 scope、内存预算或新 Cursor wire；`CUR-N2=reviewed_no_wire_delta` 保持。

目标 `CursorSessionManager` 原仅在后续注册表操作时惰性清理过期停放会话。无后续请求时，h2 流与请求内存 reservation 可超过五分钟 TTL 长期留存；固定“停放后只读 has、等待到期并检查内存”的反例先红。现停放时设置仅持弱引用的可取消任务；续接或关闭取消旧任务，到期时复核精确 entry、状态和最新活动时间，再关闭流、释放内存并清理 response/tool 索引。既有惰性 TTL 清理继续兜底；运行中会话和新停放租约不会被旧任务关闭。仅修本地会话资源生命周期，不改 AgentService h2/protobuf wire、OAuth/API Key 认证、Account/Provider/Share 单绑定、冷续接 claim、模型目录或 receipt schema v2/revision 2。离线资源反例不证明厂商真实接受性；两条 rail 真实 receipt 和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 Grok。

验证收口：无后续请求的停放会话反例先红后绿，Cursor session 定向 27/27；完整 Rust 独立串行回归为 library 3445 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Cursor 私有 receipt Node 95/95、全量后端 Node audit/smoke、冻结 Cursor reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 均通过；生成的 `docs/provider/coverage.md` 核对无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.219 Grok 图片编辑 multipart 只按分隔行切片

2026-10-11，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 与 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 media 实现；后者六项未提交 Codex 工作树内容排除。sub2api 使用标准 MIME multipart reader，文本字段不要求 Content-Type。本项只借鉴分隔与字段类型信号，不推断新的 xAI wire 或 media entitlement。

目标 `split_multipart_parts` 原把图片内容中任意 `--boundary` 字节当成分片，随后 `trim_part_data` 会剔除真实图片末尾所有 CR/LF；普通 prompt 字段缺 Content-Type 也被拒绝。内嵌 boundary 字节、尾随换行的 PNG 和无 Content-Type 的 prompt 两条固定反例先红。现只有位于完整分隔行且后缀合法的 boundary 才切片，仅删除分隔行自身的换行，图片载荷原样校验并编码；Content-Type 只对图片必需。缺结束边界显式 400，既有 MIME/签名/数量/不支持字段校验保持。只修本地 Grok 图片编辑入站解析，不改 direct-XAI 出站 endpoint/body 形状、Account/Provider/Share 单绑定、Responses/SSE/WS、remote compaction、GR-N4 版本/header 或 receipt schema。离线字节反例不能证明真实 xAI 接受性；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号证据及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 Kiro。

验证收口：两条反例先红后绿，含缺结束边界与既有 MIME/数量测试的 Grok multipart 定向 3/3；完整 Rust 独立串行回归为 library 3447 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 私有 receipt Node 159/159、全量后端 Node audit/smoke、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy 与预构建后的本地服务 smoke 均通过；生成的 `docs/provider/coverage.md` 核对无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.220 Kiro 工具 JSON 错误不得回显上游工具标识

2026-10-11，按循环只处理 Kiro。只读复核 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `ToolUseEvent` 与 `ToolJsonAccumulator`：`toolUseId`、`name` 和 JSON 分片都来自上游。参考不定义本 Server 多租户下游错误展示策略，本项不推断新的 Kiro/Amazon Q wire。

目标已隔离 `error`/`exception` 帧中的上游诊断，但 `KiroToolJsonError::Invalid/Incomplete` 原把不可信工具 ID 和名称写入错误文本。以伪凭据作工具 ID/名称的无效 JSON、未完结 JSON，在非流式和流式两条路径分别构造固定反例；旧实现先红。现保留 `TOOL_JSON_INVALID` 与 `TOOL_JSON_INCOMPLETE` 分类，对下游仅公开固定说明与本地缓冲字节数，不回显上游标识或解析器详情；有效工具事件仍原样处理，聚合、恢复与内部错误字段不变。只收紧失败展示，不改出站请求、EventStream 终态/EOF、Account/Provider/Share 与 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线恶意字段不能证明厂商真实发送；八份真实 receipt、KI-N5 专项证据及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Qoder，其推荐 TokenRouter 业务源码仍受 `project-doc` 技能门禁。

验证收口：流式/非流式两种错误的同一固定反例先红后绿；完整 Rust 独立串行回归为 library 3448 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Kiro 私有 receipt Node 492/492、全量后端 audit/smoke Node、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成的 `docs/provider/coverage.md` 核对无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。八个 auth-kind×region `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.221 Qoder usage 总数不得小于分项或通过饱和求和伪造

2026-10-11，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter，但其 `AGENTS.md` 要求本会话使用未加载的 `project-doc` 技能；读取门禁后未读取业务源码，也未运行源码差分 `--check-sources`。最新业务增量继续为 `unreviewed_skill_gate`。本项只核对目标 Server 现有 Chat usage 合同和冻结自包含 CLI oracle，不推导新的 COSY wire。

目标 `canonical_qoder_usage` 原接受显式 `total_tokens < prompt_tokens + completion_tokens`；缺 total 时用饱和加法把溢出改写为 `u64::MAX`，跨 chunk 合并亦然。原生 wrapper decoder 与非流聚合器对矛盾 total、求和溢出均错误接受；固定反例先红。现两桶求和必须精确可表示，显式 total 不得小于该和，合并后的桶值溢出也失败关闭；合法较大 total 和缺 total 时精确求和保持。Global OAuth、CN OAuth、Global PAT × Claude/Codex/Gemini 的九组 HTTP 回环还确认矛盾 total 返回 502，只向原绑定账号发一次生成请求，不触发 refresh。只修本地计量一致性，不改 COSY 出站请求/签名、Global/CN 站点与 OAuth/PAT rail、Account/Provider/Share 单绑定、唯一终态＋EOF、同账号 401 预算或 receipt schema v2/revision 2。离线异常数值不证明厂商真实用量；Global OAuth、Global PAT、CN OAuth 三份真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 CodeBuddy。

验证收口：计量定向反例旧实现先红、修复后通过；九组 HTTP 回环通过。最终完整 Rust 独立串行回归为 library 3450 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Qoder 私有 receipt Node 281/281、全量后端 Node audit/smoke、自包含 Qoder reference-delta、冻结 CLI oracle、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成的 `docs/provider/coverage.md` 已核对无需改动。没有运行会读取 TokenRouter 业务源码的 `--check-sources`；完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。三条 `RUN_REAL=0` rail 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.222 CodeBuddy 流式 usage 必须保持对象形状

2026-10-11，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 WorkBuddy SSE 聚合：参考将上游 usage 作为任意 JSON 承载，不定义目标 Server 的流/非流计量展示边界；参考工作树仅有两个未跟踪文档，均排除。本项依据目标非流聚合器现有“usage 必须是对象”合同收紧本地 decoder，不推断新的厂商 usage wire。

目标流式 decoder 原把显式数字、字符串或数组 usage 原样发往客户端，随后可被 `[DONE]` 与 EOF 记为成功；非流式聚合器则已拒绝这些值。三种非法形状的固定反例在旧实现先红。现流式在规范化之前要求非空 usage 为对象，`null` 视为缺失并移除，合法对象与显式零、CB-N6 cache 字段优先级不变。Intl/CN × Claude/Codex/Gemini × stream/non-stream 的十二组 HTTP 回环均在成功提交前返回本地 502，只向原绑定账号发一次生成请求，不触发 refresh。只修本地结果形状一致性，不改上游 endpoint/header/body、站点/Account/Provider/Share 绑定、唯一 `[DONE]`＋EOF、共享 attempt 预算或 receipt schema v2/revision 2。离线异常帧不证明真实厂商发送；两站真实 receipt、CB-N4 接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Antigravity。

验证收口：非法 usage 外层形状的单元反例先红后绿，十二组 HTTP 回环通过；完整 Rust 独立串行回归为 library 3452 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit/smoke、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成的 `docs/provider/coverage.md` 核对无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。Intl/CN 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.223 Antigravity replay 工具结果 ID 与名称共同绑定旧调用

2026-10-11，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：其 `functionResponsesCanRestoreID` 对按 ID 关联的工具结果还核对非空名称；另一推荐 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 只作既有 Gemini 签名保真差分背景。参考的特殊 `unknown` 名称规则不属于目标现有合同，不照搬；不推断新的 Google wire。

目标 `response_matches_calls` 原在结果 ID 命中后忽略名称，非空错误 ID 又可回退到同名旧调用。缓存 `read_file`，续接却给同 ID 的 `write_file` 结果时，旧实现插入带旧签名的 `read_file` 调用；固定反例先红。现非空结果 ID 必须精确命中缓存调用，提供的非空名称也必须匹配；无 ID 的同名关联不变。另覆盖错误 ID／同名反例，以及 `antigravity_oauth`、`agy_oauth` 的 Gemini HTTP 回环：改名结果原样出站，旧签名不注入，只向当前绑定账号发送一次续接请求。只收紧本地 AG-D3 replay 身份关联，不改出站 endpoint/header/body、Account/Provider/Share scope、AG-N6 requestType、终态提交、compaction disabled 或 receipt schema v2/revision 2。离线反例不能证明 Google 实际发送该形状；两条真实 OAuth rail receipt、AG-N6/AG-D3 专项接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Claude。

验证收口：单元反例旧实现先红、修复后绿，双 rail Gemini HTTP 回环通过；完整 Rust 独立串行回归为 library 3454 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt Node 165/165、全量后端 Node audit/smoke 1909/1909、冻结 reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 和差分空白检查通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。`antigravity_oauth`、`agy_oauth` 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.224 Claude SSE 容量上限按完整事件与剩余半帧分别执行

2026-10-11，按循环只处理 Claude。只读复核 `Claude/AGENTS.md` 推荐且干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `claude_executor_stream.go::validateClaudeStreamingResponse`：参考验证 SSE `data:` 行的 JSON 与基本生命周期，不定义目标 Server 8 MiB 单事件上限。本项依据目标既有 Anthropic 语义守卫合同修正容量检查，不推断新的厂商 wire。

目标 `AnthropicSseInspector::push_inner` 原在追加整个网络块后只要找到任一完整事件边界就跳过未完成事件的超限预检。固定“同块完整 `message_start` + 超过 8 MiB 的下一条半帧”反例先红：旧实现返回正常 `Lifecycle`，未立即拒绝超限半帧。现先排空并逐条检查完整事件，再对剩余未完成事件执行相同上限；多条各自小于上限、合计超过 8 MiB 的完整事件仍可通过。仅修本地 SSE 容量失败时机，不改变出站 Messages/count_tokens、Account/Provider/Share 绑定、CL-N7/CL-N8、429 scope、终态语义、receipt schema v1/revision 1 或已关闭能力。离线分块不能证明 Anthropic 实际发送该形状；五份 operation 真实 receipt、thread/beta 厂商接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Codex。

验证收口：超限半帧单元反例旧实现先红，修复后与多完整事件正例转绿，Anthropic inspector 定向 38/38；完整 Rust 独立串行回归为 library 3456 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 252/252、全量后端 Node audit/smoke 1909/1909、冻结 Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.225 Codex 错误帧优先于后续 incomplete 终态

2026-10-12，按循环只处理 Codex。只读复核 `Codex/AGENTS.md` 推荐且干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`；前者将 `error` 识别为失败事件。官方 OpenAI Docs 的 Responses streaming 指南区分 `error` 与完成事件，却不定义异常并列序列的优先级；本项沿用目标 Server 已有的错误后 completed 失败和非流聚合合同，不推断厂商新 wire。

目标 `ResponsesSseInspector` 原仅在暂存 `error` 后看到 `response.completed` 时提升错误；若后续是 `response.incomplete`，就把错误降格为普通未完成终态。跨块固定反例先红。现两种后续终态都必须保留先到错误；无前置错误的正常 incomplete 继续报告未完成，非流聚合器原有失败门槛由新增回归锁定。Codex OAuth HTTP 回环证明已提交业务输出后仍以公开 `error` + 合成 `response.failed` 收尾，不发送伪 incomplete 或 `[DONE]`，usage 为 `provider_failed`；容量拒绝码继续按既有策略对客户端映射为 `server_error`，内部失败分类不变。仅修本地普通 Responses HTTP SSE 终态优先级，不改出站 Responses/Images/WS、Account/Provider/Share 单绑定、同账号恢复预算、CX-N13/CX-D3 或 receipt schema。离线冲突帧不证明真实上游会这样发送；`gpt_6_astra`、三个 Image 2.5 exact-model 与 WS prewarm 的五份真实 receipt、厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Cursor。

验证收口：错误后 incomplete 的流式单元反例旧实现先红、修复后绿，非流聚合回归与 Codex OAuth 跨网络块 HTTP 回环通过；完整 Rust 独立串行回归为 library 3459 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205、全量后端 Node audit/smoke 1909/1909、冻结 Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 核对同步，无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.226 Cursor 同 ID Running/parked 仲裁边界复核

2026-10-12，按循环只处理 Cursor。只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `cursorSessionManager.ts`：其 `findByToolCallIds` 只找 parked 会话并选首个匹配，不定义本 Server 同作用域重复 ID 的安全仲裁；22 项未提交 Codex 工作树内容继续排除，`CUR-N2=reviewed_no_wire_delta` 保持。目标 `resolve_index_binding` 同时计算 Running 与 parked 存活候选，初看可能因 Running 占位而拒绝唯一可获取的 parked 会话；但调用路径同时处理工具结果和重复续接，Running owner 可能正是重复结果的目标。若按可获取性排除它，会把结果误接到另一个 parked h2 流。故保留生产逻辑，不把可疑点误修成跨会话路由风险。

新增确定性双索引回归：同 scope 的 Running、parked 两个不同 conversation 绑定相同 response/tool-call ID 时都不可解析；Running 关闭并清理索引后，parked 恢复唯一可解析。`docs/provider/cursor.md` 与 `PROTOCOL_EVIDENCE.md` 已写清该保守边界。定向 Rust 通过；完整独立串行 Rust 为 library 3460 passed / 1 ignored，其他目标 124 + 1 + 1 passed；Cursor 私有 Node 95/95、全量后端 Node audit/smoke 1909/1909，冻结 Cursor reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、本地服务 smoke 与差分空白检查通过。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发真实请求；离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。两份真实 receipt、厂商接受性与当前未提交工作树的 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。本轮未提交，下一单类别为 Grok。

### 18.227 Grok SSE ping 不得吞掉错误载荷

2026-10-12，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 推荐且干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 以及 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交实现；后者六项未提交 Codex 工作树改动排除。参考证明严格 Responses 客户端需要把厂商计费／保活 ping 规范化，也存在无 `type` 的费用 JSON；但不保证异常 ping 载荷的厂商形状。GR-N4 同账号版本/header 接受性不借此提升或改 wire。

目标 `grok_sse_event_is_ping` 原把任意无 `type` 的 JSON（或 `type=ping`）当作保活，导致 `event: ping` 携带 `{"error":{"message":"upstream failed"}}` 时错误被改写成 `: ping`。单元反例旧实现先红；现在共享 Responses 语义分类先识别错误帧、失败、不完整与成功终态，这些载荷保持原样由主门禁判定。普通无 `type` 的 `{"cost":"0"}` 仍按 ping 兼容。HTTP 回环在已完成搜索和成功终态后接错误 ping，主门禁于响应提交前返回 502，不写搜索能力证据。仅修 Grok OAuth Responses SSE 入站保真，不改出站 endpoint/header/body、HTTP JSON/WS、Account/Provider/Share 固定绑定、同账号恢复预算、remote compaction disabled 或 receipt schema v1/revision 1。离线冲突帧不证明 xAI 实际会这样发送；inference、media、remote_compaction 三份真实 receipt、GR-N4 同账号厂商接受性与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Kiro。

验证收口：错误 ping 单元反例旧实现先红、修复后绿，HTTP 回环确认出站前 502 与无搜索证据；完整 Rust 独立串行为 library 3461 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 私有 receipt Node 159/159、全量后端 Node audit/smoke 1909/1909、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 核对无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。三项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.228 Kiro EventStream 错误诊断隔离上游事件与 header 名

2026-10-12，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/kiro/model/events/base.rs`：未知事件类型被归入固定 `Unknown`，但参考不定义本 Server 面向客户端的诊断文本。本轮只采用上游事件名不可信这一边界信号，不推断新 Kiro/Amazon Q EventStream wire。

目标已隔离上游 `error/exception` 和工具 JSON 标识，却在 `KiroToolJsonError::from` 中对 `InvalidEventPayload`、`DuplicateHeader` 直接显示内部错误；前者包含原始 `:event-type` 与解析器文字，后者包含原始 header 名。以伪凭据作事件/header 名的反例旧实现先红。现保持 `KIRO_EVENT_STREAM_INVALID` 分类、内部 `WireError` 原值以及其他结构/CRC/容量诊断，仅对客户端把这两类改成固定说明。真实编码帧的非流转换与流式桥接均验证不回显；合法工具和事件不变。此项只修本地失败展示，不改出站请求、EventStream 帧解码/终态/EOF、Account/Provider/Share 与八种 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线恶意 header 不证明厂商实际发送；八份真实 receipt、KI-N5 独立计量证据与当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一类别为 Qoder。

验证收口：伪凭据诊断单元反例旧实现先红、修复后绿；真实编码帧的流式/非流式回归通过。完整 Rust 独立串行为 library 3463 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Kiro 私有 receipt Node 492/492、全量后端 Node audit/smoke 1909/1909、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 核对无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。八个 auth-kind×region `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.229 Qoder 工具调用 ID 不透明身份保真

2026-10-12，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter；其 `AGENTS.md` 要求先加载当前会话没有的 `project-doc` 技能。未读取推荐仓业务源码，也未运行访问外仓源码的 `--check-sources`；最新参考业务增量继续为 `unreviewed_skill_gate`。本项依据目标 Server 既有工具分片精确 ID 合同和冻结的自包含 CLI oracle，不推断新 COSY wire。

目标 `canonical_qoder_tool_call` 原将上游 `" call-a "` ID 裁剪为 `"call-a"`，而非流聚合器会按同一工具索引精确比较后续 ID，导致两个不同上游身份可能被误合并。保留原 ID 的固定反例在旧实现先红；修复后只用裁剪结果判断 ID 是否全空，非空原值原样输出，首尾空白与无空白 ID 的跨分片冲突被拒绝。工具名称原有裁剪、全空白 ID 缺失、空 placeholder、工具 index、终态及聚合器精确比较均不变。只修本地身份保真，不改 COSY 出站 endpoint/header/body/签名、Global/CN 站点和三 credential rail、Account/Provider/Share 单绑定、共享 attempt/401 预算或 receipt schema v2/revision 2。离线带空白 ID 不证明厂商实际发送；三份真实 rail receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审和当前未提交工作树的 committed-target observation 均缺。最终整体 review 未完成，未做 UI 自动化或部署测试；下一单类别为 CodeBuddy。

验证收口：定向反例旧实现先红、修复后通过；完整 Rust 独立串行 library 3464 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Qoder 私有 receipt Node 281/281、冻结 CLI oracle 9/9、自包含 reference-delta、全量后端 Node audit/smoke 1909/1909、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings` 与差分空白检查通过；生成 coverage 无需改动。本地服务 smoke 前两次因脚本默认编译环境与预构建的 `CARGO_INCREMENTAL=0` 不一致、超过 30 秒启动等待而失败；相同环境预构建后重跑通过。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。Global OAuth、Global PAT、CN OAuth 三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内部测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.230 CodeBuddy 缺失 index 时按完整工具 ID 找回

2026-10-12，按循环只处理 CodeBuddy。只读复核 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a`：从上次已审 `a6502521e1e509a4257153025c642240c39cd54b` 到当前 HEAD，`internal/providers/workbuddy/` 没有源码差异；工作树中未跟踪的 `proxy.html`、`proxy.md` 排除。其已提交聚合器按显式 index 合并工具分片，不定义本 Server 的无 index 回退规则。本项依据目标 Server 自己的“首帧 index 可缺、同 ID 精确聚合”合同，不推断新的 WorkBuddy wire。

目标 `CodeBuddyChatSseAggregator` 原保存工具 ID 原值，却在后续分片缺少 index 时先裁剪 ID 再查找。并行工具首帧的 index 0 为 `call-a`、index 1 为 `" call-b "`，第二帧仅有 `" call-b "` 且排在首位时，旧实现找不到原 index 1，按位置误选 index 0，随后错误拒绝“ID 跨分片变化”；固定反例先红。现仅以裁剪结果判定 ID 是否全空，非空 ID 以原值精确查找；显式 index、真正无 ID 的位置回退、跨分片 ID 冲突拒绝和参数拼接保持。该改动只修本地非流工具身份，不改 WorkBuddy 出站 endpoint/header/body、Intl/CN 站点、Account/Provider/Share 单绑定、唯一 `[DONE]`+EOF、同账号恢复预算、receipt schema v2/revision 2 或 CB-N4 disabled。离线空白 ID 不证明厂商真实发送；两站真实 receipt、CN `deepseek-v4.1-flash` 目录/接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试；下一单类别为 Antigravity。

验证收口：定向反例旧实现先红、修复后通过；完整 Rust 独立串行 library 3465 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit/smoke 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、相同构建环境预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。Intl/CN 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内部测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.231 Antigravity 思考 token 的 OpenAI usage 明细保真

2026-10-12，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐且干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 最近已提交的 `ab151f5d`：其 OpenAI Chat 转换把 `thoughtsTokenCount` 加入 completion 总数，并显示 `reasoning_tokens`。`dfe0a0ef` 的无工具流省略 `tool_calls:null` 在目标实际流桥未发现对应问题，不据此改生产代码。另一推荐 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 保留既有签名差分背景。参考只提供计量投影线索，不证明 Google 原始用量口径或新 wire。

目标实际 Gemini→Anthropic→OpenAI 流桥已把候选与思考 token 合并进输出总数，但丢失显式思考明细；Responses `output_tokens_details.reasoning_tokens` 与 Chat `completion_tokens_details.reasoning_tokens` 的固定反例在旧桥先红。目标辅助 Gemini→OpenAI usage 转换则连 completion 总数也只算候选；候选 5／思考 7、仅思考 7 的反例先红。现 OpenAI 目标的流式桥在终态 usage 上附加显式思考明细，通用 Anthropic→OpenAI 转换保留该字段；非流 Antigravity/AGY 双 rail 的 Chat 与 Responses 也保留相同明细。辅助转换的 completion 总数改为候选＋思考，缺字段和显式零仍按既有兼容规则处理。Claude 原生 Messages usage 保持原形，没有增加 OpenAI 专用字段。只修本地计量投影，不改出站 endpoint/header/body、Account/Provider/Share 单绑定、AG-N6 requestType、AG-D3 replay/compaction、终态/EOF 或 receipt schema v2/revision 2。离线计量反例不证明厂商真实用量；双 rail 真实 receipt、AG-N6/AG-D3 专项接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试；下一单类别为 Claude。

验证收口：辅助转换与实际流式桥的反例先红后绿，Antigravity/AGY × Chat/Responses 非流式回归通过，Claude 原生 usage 形状断言通过。完整 Rust 独立串行 library 3472 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt Node 165/165、全量后端 Node audit/smoke 1909/1909、冻结 reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、相同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。`antigravity_oauth`、`agy_oauth` 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内部测试、缺真实输入与未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.232 Claude deferred tool 的默认缓存断点避让

2026-10-12，按循环只处理 Claude。只读复核 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b`：已提交 `claude_executor_cloaking.go::injectToolsCacheControl` 与 `caching_verify_test.go` 明确在默认 tool 断点注入时跳过尾部 `defer_loading`，全部 deferred 则不加 tool 断点。新近的原生 `tool_search`、能力开关、不可用时回退和每请求固定开关实现（`aeec5e97`、`52992659`、`956c3da1`、`29bd0415`、`ce518c9b`）也已与目标 Responses→Claude 路径对照：目标当前将内建搜索映射为兼容式 `tool_search` 函数，尚无原生能力开关、按模型/credential 判定或请求内固定决策。原生路径是独立能力扩展，不能把此轮缓存修复宣称为原生搜索落地；后续应先冻结官方/参考 wire fixture，再设计保守 opt-in、能力拒绝回退、完整流/非流映射和热更新期间请求一致性，真实账号 receipt 前保持关闭。

目标 `normalize_anthropic_cache_control` 原在无 system/既有 tool 断点时给最后一个工具自动加 `cache_control`，即使该工具 `defer_loading=true`。固定反例旧实现先红。现只对默认注入逆序找最后一个非 deferred 工具，并兼容目标已有 beta 判定承认的 `custom.defer_loading=true`；全部 deferred 时不加 tool 断点，最近 user message 的滚动断点照常注入。客户端显式 marker 不受此规则改动；确认 Native CLI 的最小修改路径仍不自动注入。不改 Claude OAuth endpoint/header、Account/Provider/Share 单绑定、CL-N7/CL-N8、模型目录、receipt schema v1/revision 1 或 CCH 的最终签名顺序。离线 fixture 不证明真实 Anthropic 接受性；五份真实 operation receipt、thread/beta/原生工具接受性和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 Codex。

验证收口：默认断点反例在旧实现先红，修复后缓存模块 8/8；完整 Rust 独立串行 library 3474 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 独立复跑 252/252，全量后端 Node audit/smoke 1909/1909；首次与 Rust 编译并行的 Claude 专项运行出现 16 项夹具失败，独立复跑和全量复跑均未复现，本轮未把它归因于缓存逻辑。冻结 Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、相同构建环境预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。五个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过其内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.233 Codex previous-response 工具身份按原始 ID 与类型配对

2026-10-12，按循环只处理 Codex。只读复核 `Codex/AGENTS.md` 推荐、工作树均干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 与 `codex2api@30656549b7f5806af1db99ab2212cabe87cb8328`。后者新提交 `c3635bb7`、`7c210fa5` 为原生 WS 工具续链增加上下文快照及连接保留；`responses_ws_context.go::responsesWSContextItemKey` 与 `response_cache.go::responseContextPairType` 以原始 `call_id` 和对应工具类型识别配对。目标已有固定 Share/账号的有界 previous-response 工具缓存与 HTTP/WS 共用展开路径，不引入参考的跨账号 replay、完整对话或连接保留策略；目标 `supports_search_tool` 当前只解析 Codex OAuth manifest，参考新提交 `11f04c20` 的 Claude 原生 route 宣告依赖上一轮仍未开放的原生能力，故本轮不把 Claude 兼容函数桥广告为原生工具搜索。

目标 `sanitize_replayable_tool_item` 和 `replayable_tool_item_key` 原裁剪 `call_id`，使 `" call-a "` 与 `"call-a"` 错误碰撞；`requires_previous_response_tool_context` 也裁剪 ID，且只按 ID 判断当前 tool output 有无配对，`function_call` 可以错误满足同 ID 的 `custom_tool_call_output`。原始 ID 和异类结果两条固定反例分别先红。现在只用裁剪结果排除全空 ID，缓存、注入、判重和缺历史门禁均按原始 ID；结果必须与相同调用类型精确配对。当前 input 中真正完整的一对不额外依赖旧缓存，缺配对且有 `previous_response_id` 时仍按既有 409 fail closed。此项只修本地工具续链身份，不改出站 Responses/Images/WS wire、Account/Provider/Share 固定绑定、同账号恢复预算、CX-N13/CX-D3、WS prewarm disabled 决策或 receipt schema v1/revision 1。离线含空白 ID/异类结果不能证明真实厂商发送；Astra、三个 Image 2.5 与 WS prewarm 五份真实 receipt、厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；下一单类别为 Cursor。

验证收口：两条身份反例先红后绿；previous-response 定向 Rust 17/17，WebSocket 缺历史时 409 回归通过。完整 Rust 独立串行 library 3480 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205、全量后端 Node audit/smoke 1909/1909、冻结 Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、相同构建环境预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 无需改动。构建中发现工作区已有 `ShareUpstreamQuota` 新增可选 `banked_reset`，而 Ollama 投影旧初始化缺字段；只补 `src/state.rs` 一行 `None` 解除编译错误，并机械同步 Phase 0 writer 指纹，不改变该投影业务。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。五个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，无真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过其内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.234 Cursor 会话键计入请求内存预算

2026-10-12，按循环只处理 Cursor。只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 session manager：以 conversation ID 保存同一 h2 流，默认限制 100 个会话、五分钟空闲 TTL；其 22 项未提交 Codex 工作树内容仍排除。参考未定义目标 Server 的多租户请求内存预算，本项不推断新 Cursor wire，`CUR-N2=reviewed_no_wire_delta` 保持。

目标 `try_build_plan` 在非 Responses 兼容入口也提取客户端 `previous_response_id`；无匹配会话时，`resolve_session_key` 会以该原值构造 conversation key。注册表和 `CursorSession` 各保留一份键字符串，但 `session_retained_bytes` 原未计入两份 scope/conversation ID。4096 字节与短 ID 的同内容会话内存差分反例旧实现先红。现计入两份字符串容量与注册表键固定开销，由现有 request-memory reservation 执行容量拒绝；入口取值、键精确匹配、OAuth/API Key scope、parked 续流、出站 protobuf、Account/Provider/Share 单绑定和 receipt schema v2/revision 2 均不变。离线长 ID 不证明厂商接受性，OAuth/API Key 两份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一单类别为 Grok。

验证收口：会话键内存差分反例在旧实现先红、修复后绿；非 Responses 客户端 ID 到会话键的入口回归通过。完整 Rust 独立串行为 library 3482 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Cursor 私有 receipt Node 95/95、全量后端 Node audit/smoke、冻结 Cursor reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、相同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 核对无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.235 Grok 图片编辑 multipart 头部以最早空行结束

2026-10-12，按循环只处理 Grok。只读复核 `Grok/AGENTS.md` 推荐、工作树干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`，以及 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 `grok_media.go`：后者使用标准 multipart reader；其六项未提交 Codex 内容继续排除。参考只提供分帧线索，不定义目标 direct-XAI 的 entitlement 或新 wire，GR-N4 同账号版本/header 决策保持待证。

目标图片编辑已支持 LF-only 和 CRLF part 头部，但 `split_part_headers` 原在整个 part 先找 CRLFCRLF，找不到才找 LFLF。LF-only 头部后带 PNG 签名的图片数据内有 CRLFCRLF 时，旧实现会跨越真实头部结束，截坏图片并在出站前错误返回 400。实际图片编辑入口的固定反例先红。现比较两个候选分隔符的位置，选最早空行；图片载荷仍原样通过既有签名、MIME、大小与数量校验。完整 boundary 行、末尾换行、文本字段、未支持字段、出站 JSON、Account/Provider/Share 单绑定、Responses/SSE/WS、remote compaction disabled 和 receipt schema v1/revision 1 都不变。离线图片反例不能证明 xAI 真实接受性；inference、media、remote_compaction 三份真实 receipt、GR-N4 同账号证据和当前未提交工作树的 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一单类别为 Kiro。

验证收口：实际图片编辑入口的 LF-only 头部＋图片内 CRLFCRLF 反例旧实现先红、修复后绿；Grok multipart 定向 4/4。完整 Rust 独立串行为 library 3483 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 私有 receipt Node 与全量后端 Node audit/smoke、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 核对无需改动。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。inference、media、remote_compaction 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过其内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.236 Kiro 工具顶层组合式 schema 的安全展开

2026-10-12，按循环只处理 Kiro。只读复核 `Kiro/AGENTS.md` 推荐且干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`：参考将上游 `TOOL_SCHEMA_INVALID`（如顶层 `oneOf`/`allOf`/`anyOf`）视为客户端请求错误，不跨账号重试。目标为兼容 Claude Code 原先会将顶层组合式 schema 展开为首个显式 object 分支，但多对象分支因此静默丢失合法工具参数形状；固定反例旧实现先红。现出站前拒绝多个分支、无显式对象、畸形组合及根级约束冲突，保持恰好一个显式对象分支的兼容展开。该分支中的 Claude Code 内建字段名递归映射，`Read.pages` 禁止规则同步进入分支，避免 schema 与输入不同名或绕过原有兼容门槛。

此项只收紧本地工具请求转换，不改 Kiro/Amazon Q 出站 endpoint、credential/region rail、EventStream、Account/Provider/Share 固定绑定、同账号 401 预算、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。参考及离线反例都不能证明真实厂商接受性；八份 auth-kind×region 真实 receipt、KI-N5 专项证据和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一单类别为 Qoder，其推荐 TokenRouter 业务源码仍受当前会话缺失的 `project-doc` 技能门禁。

验证收口：多对象分支反例旧实现先红，新实现及单一对象字段映射、`Read.pages` 分支拒绝等 Kiro 定向 Rust 100/100；完整 Rust 独立串行为 library 3487 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。Kiro 私有 receipt Node 492/492、全量后端 Node audit/smoke 1909/1909、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、本地服务 smoke 和差分空白检查均通过。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。八个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不能计为发布通过。本轮未提交，不追加 committed-target observation。

### 18.237 Qoder 单 chunk 多 choice 不得合并为一条回答

2026-10-12，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter，但其 `AGENTS.md` 要求先使用当前会话未加载的 `project-doc` 技能；读取门禁后没有读取业务源码或运行外仓源码差分审计，最新业务增量保持 `unreviewed_skill_gate`。本项只检查目标既有的单 choice index 0 合同和冻结 CLI oracle，不假定新的 COSY wire。目标原来逐个校验 `choices[].index=0`，却允许同一 chunk 带两个 index 0；流式转发会发布两条同身份 delta，非流聚合会把两份内容拼成一条回答。固定反例旧实现先红。现 decoder 规范化与非流聚合均在业务转发前要求一个 chunk 至多一个 choice；空 choices 的 usage-only chunk 和跨 chunk 的正常续接保留。

此项只修本地响应身份，不改 COSY 出站 endpoint/header/body/签名、Global/CN 站点和 OAuth/PAT rail、Account/Provider/Share 固定绑定、唯一终态+EOF、同账号 401 恢复预算或 receipt schema v2/revision 2。Global OAuth、Global PAT、CN OAuth 的三份真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮离线验证收口后下一类别为 CodeBuddy。

验证收口：双 choice 固定反例旧实现先红、修复后绿；Global OAuth、CN OAuth、Global PAT × Claude/Codex/Gemini 九组回环均在成功提交前返回 502，只向原绑定账号发一次生成请求且不触发 refresh。Qoder 定向 Rust 99/99。首次完整 Rust 独立串行回归只有既有 `accepted_provider_commit_survives_caller_cancellation` 时序测试失败；该测试单独重跑通过，未修改跨类别代码。随后无其他测试负载的完整复跑为 library 3489 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`RUST_MIN_STACK=16777216`）。Qoder 私有 receipt 与 CLI oracle Node 共 290/290，全量后端 Node audit/smoke 1909/1909，自包含 Qoder reference-delta、冻结 CLI oracle、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、本地服务 smoke 与差分空白检查均通过。未运行受 `project-doc` 门禁约束的 TokenRouter 业务源码 `--check-sources`，完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。三条 `RUN_REAL=0` rail 各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.238 CodeBuddy 单 chunk 多 choice 不得拼接成一条回答

2026-10-12，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 WorkBuddy SSE：非流聚合器只读取首个 choice，流复制器宽松处理全部；从此前已审提交至当前 HEAD，WorkBuddy 业务源码无差异，两个未跟踪文档排除。参考不定义目标 Server 的严格多租户失败合同。本项目流式 sanitizer 与非流聚合器原逐项验证 `index=0`，却允许同一 chunk 带两个 index 0；前者转发两条同身份分支，后者拼成一条回答。固定反例旧实现先红。现两条路径在业务输出前拒绝多 choice；空 choices 的独立 usage 帧、跨 chunk 的单 choice 续接及原有 index 兼容保持。

仅修本地响应身份，不改 WorkBuddy 出站 endpoint/header/body、Intl/CN 站点、Account/Provider/Share 固定绑定、唯一 `[DONE]`+EOF、同账号 401 恢复预算、receipt schema v2/revision 2 或 CB-N4 disabled 决策。两站真实 receipt、CN `deepseek-v4.1-flash` 目录/接受性和未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。本轮验证收口后下一单类别回到 Antigravity。

验证收口：双 choice 单元反例旧实现先红、新实现转绿；Intl/CN × Claude/Codex/Gemini × stream/non-stream 十二组 HTTP 回环均在成功提交前返回 502，只向原绑定账号发一次生成请求且不触发 refresh。CodeBuddy 关键词 Rust 98/98；首次以默认线程栈运行触发既有 CodeBuddy provider-test 栈溢出，按既有验证约定设置 `RUST_MIN_STACK=16777216` 后通过，未修改跨类别测试。完整 Rust 独立串行为 library 3491 passed / 1 ignored，其他目标 124 + 1 + 1 passed。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit/smoke 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、本地服务 smoke 与差分空白检查均通过。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。Intl/CN 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.239 Antigravity 工具名称边界空白的最终出站校验

2026-10-12，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@b601f5e7375343343922ccab5e50653bc3450c3b` 已提交工具名映射：参考按原始名称建立 request-local sanitized map，不把裁剪后的名称与原始名称混用；另一推荐 `Antigravity-Manager` 的仓库约定已核对，本项不借用其账号池、UI 或其他运行策略。目标 Server 的最终 wire 规范化却先裁剪声明名做匹配，也裁剪调用/结果名做 ID 关联，再按未裁剪原值改写名称；`" lookup "` 可留在声明中或与调用产生不同 wire 名。声明、调用、结果三类固定反例旧实现先红。现最终出站首步统一拒绝这些字段及 allowedFunctionNames 的首尾空白，再做 ID 关联、名称映射和函数结果邻接；名称内部空格继续按既有规则映射，严格碰撞拒绝保持。实际 Gemini v1internal 清洗入口确认本地 400。

仅收紧本地工具身份，不改 Antigravity/AGY 出站 endpoint/header/body 的其他字段、Account/Provider/Share 单绑定、AG-N6 requestType、AG-D3 replay/compaction、终态与 receipt schema v2/revision 2。双 OAuth rail 的真实 receipt、AG-N6/AG-D3 专项厂商接受性和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Claude。

验证收口：声明、调用、结果名称边界空白的固定反例先红后绿，实际 Gemini v1internal 清洗入口的 400 回归通过。完整 Rust 独立串行回归为 library 3493 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）；Antigravity 私有 receipt Node 165/165、全量后端 Node audit/smoke 1909/1909 通过。冻结 Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过，生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。`antigravity_oauth`、`agy_oauth` 两条 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.240 Claude 托管 tool_search 的执行位置栅栏

2026-10-12，按循环只处理 Claude。只读复核 `Claude/AGENTS.md` 推荐、工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b`：`isResponsesClientToolSearch` 明确只把 `execution=client` 作为可经 Claude 兼容桥执行的搜索，hosted/server search 无法由 Claude 代执行；参考还在原生能力不可用时剥离搜索并按请求固定开关。目标 `openai_responses_to_anthropic` 原忽略搜索的 `execution`，把显式 hosted 声明当作普通 `tool_search` 函数；固定反例先红。现出站转换前拒绝显式非客户端执行的声明和历史 call/output，缺省 execution 的既有兼容输入及显式 `client` 保持。Claude API Key/OAuth 两类 Provider 的 Codex Responses 实际转换入口确认本地 400。

此项只修 Responses→Anthropic 兼容桥的执行语义，不启用原生 tool-search，不改 Claude OAuth endpoint/header/CCH、Account/Provider/Share 单绑定、其他 Provider 的 Responses→Chat 路径、缓存断点、模型目录或 receipt schema v1/revision 1。原生能力开关、模型/credential 判定、回退、完整流/非流映射及请求内热更新一致性仍按 §18.232 的独立能力计划，真实账号 receipt 前不开放。五份真实 Claude operation receipt、原生工具接受性与当前未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一类别为 Codex。

验证收口：显式 hosted 声明的固定反例在旧转换上先红，修复后声明、历史 call/output 与实际 Claude API Key/OAuth 入口回归均通过；第一次修复后定向编译因新增测试少传 `gemini_path` 且对无 `Debug` 的成功值调用 `expect_err` 失败，修正测试调用后两项定向通过，未更改生产逻辑以迁就编译。完整 Rust 独立串行回归为 library 3495 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 252/252、全量后端 Node audit/smoke 1909/1909、冻结 Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。五个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.241 Codex tool_search_output 的 previous-response 配对

2026-10-12，按循环只处理 Codex。只读复核 `Codex/AGENTS.md` 推荐且工作树干净的 `codex2api@30656549b7f5806af1db99ab2212cabe87cb8328` 和 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b`。前者 `responseContextPairType` 以调用类型及原始 `call_id` 配对，支持 `tool_search_call_output`；其其他 namespace/preflight 路径也认识 `tool_search_output`，但缓存配对表尚未覆盖该别名，不能照抄作为完成证据。目标自身 Responses 工具转换已产出/消费 `tool_search_output`，previous-response 缓存却只把 `tool_search_call_output` 当成搜索结果。带 `previous_response_id` 的缺历史搜索结果因此绕过发网前 409；固定反例旧实现先红。

现用单一结果→调用类型映射把两种搜索结果均精确配到 `tool_search_call`，只以原始 `call_id` 判断身份；已有完整配对不误报，结果可以进入有界缓存，已有缓存可注入调用，WS 缺缓存按既有 `response_context_unavailable` 返回 409。只修本地续链门禁与缓存识别，不扩大保存范围到完整对话，不改 Codex OAuth Responses/Images/WS 出站 wire、Account/Provider/Share 单绑定、同账号恢复预算、CX-N13/CX-D3、WS prewarm disabled 决策或 receipt schema v1/revision 1。离线别名形状不证明真实上游会发送；Astra、三个 Image 2.5 和 WS prewarm 五份真实 receipt、厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Cursor。

验证收口：`tool_search_output` 缺调用的固定反例在旧实现先红，修复后对应类型/原始 ID 配对、当前轮完整配对、有界缓存注入和 WS 缺历史 409 回归通过。完整 Rust 独立串行回归为 library 3496 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205、全量后端 Node audit/smoke 1909/1909、冻结 Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。五个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.242 Cursor 二级续接索引计入请求内存预算

2026-10-12，按循环只处理 Cursor。只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `cursorSessionManager.ts`：它以 conversation ID 保存同一 h2 流，保留 pending tool-call 映射，关闭时清理；其 22 项未提交 Codex 工作树内容全部排除。参考不定义本 Server 的多租户二级索引或请求内存预算，本轮只采用会话索引生命周期信号，不推断新的 Cursor wire，`CUR-N2=reviewed_no_wire_delta` 保持。

目标 `response_sessions` 和 `tool_call_sessions` 为每个绑定复制 scoped ID、会话键及候选列表，原 `session_retained_bytes` 却只计主会话键和 pending tool-call map。4 KiB response ID 绑定后预算不增长的固定反例在旧实现先红。现绑定前按二级索引键、候选 Vec 和哈希槽保守预留请求内存；容量不足时在索引写入前拒绝，失败不会留下可续接的半成品索引，关闭或过期随会话释放 reservation。重复绑定可能多计，但不会少计或改变歧义仲裁。流式与非流式 response 入口在失败时关闭会话；工具调用走既有错误终态处理。只修本地资源边界，不改 OAuth/API Key scope、出站 AgentService protobuf、Account/Provider/Share 单绑定、100 会话容量、冷续接 claim 或 receipt schema v2/revision 2。离线容量反例不证明 Cursor 真实接受性，双 rail 真实 receipt 与当前未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一单类别为 Grok。

验证收口：4 KiB 索引未计费反例旧实现先红，修复后 response/tool-call 两类绑定均计费；两类预算不足均在索引写入前失败且关闭后释放 reservation。新增两项定向 Rust 通过；首次编译仅因新增测试的 `MutexGuard` 借用类型不匹配失败，修正测试借用后生产逻辑未变。完整 Rust 独立串行为 library 3498 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Cursor 私有 receipt Node 95/95、全量后端 Node audit/smoke 1909/1909、冻结 Cursor reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 核对无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.243 Grok 图片编辑 multipart 的标准 MIME boundary 解析

2026-10-12，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 Console media 路径，以及 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 `grok_media.go`：前者接收结构化图片输入，后者使用标准 MIME parser 读取 multipart boundary；后者六项未提交 Codex 工作树内容排除。参考不定义目标 direct-XAI 接受性，GR-N4 同账号版本/header 决策继续 `live_pending_no_wire_change`，不推导新上游 wire。

目标图片编辑入站原按小写字面 `boundary=` 拆 Content-Type；合法的大小写变体 `Multipart/Form-Data; Boundary="..."` 被错误拒绝，而带 boundary 的伪媒体类型 `multipart/form-dataevil` 可能通过。实际转换入口正反例中合法输入在旧实现先红。现复用已有 `multer::parse_boundary` 对媒体类型和参数做标准 MIME 解析，继续保留 boundary 长度/字符限制、分隔行及图片字节保真、最多三图、签名/MIME/大小和不支持字段拒绝。仅修本地入站解析，不改出站 JSON/endpoint、Account/Provider/Share 单绑定、Responses/SSE/WS、remote compaction disabled 或 receipt schema v1/revision 1。离线 MIME 反例不证明 xAI 真实接受性；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号证据和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；验证收口后下一类别为 Kiro。

验证收口：合法 MIME 大小写变体固定反例在旧实现先红，修复后与伪媒体类型拒绝、现有图片签名/数量/字节保真等 Grok multipart 定向 5/5 通过。完整 Rust 独立串行为 library 3499 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 私有 receipt Node 159/159、全量后端 Node audit/smoke 1909/1909、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 核对无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。inference、media、remote_compaction 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.244 Kiro Claude Code 工具别名冲突不得静默覆盖

2026-10-12，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/anthropic/converter.rs`：`Write` 等内建工具输入的别名采用明确的 `take_first` 优先级；参考不定义目标 Server 的双键冲突策略，也不能证明双键输入的真实接受性。目标原逐键重写，`Write` schema 同时声明 `file_path` 与 `path` 时会静默丢失一个 property，并使 `required` 出现两个 `path`；历史工具输入双键还可能因对象顺序而改写参数含义。两条固定反例在旧实现先红。现仅对 Claude Code 内建工具模式，于发网前检查 schema 的属性、必填项、嵌套 items/组合分支和历史 `tool_use.input` 的映射后键冲突，返回本地 400；单别名输入和 Raw 模式不变。

此项仅修本地请求转换，不改 Kiro/Amazon Q endpoint/header/body 的其他部分、EventStream、Account/Provider/Share 与八种 auth-kind×region 单绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线双键测试不证明厂商实际发送该形状；八份真实 receipt、KI-N5 独立计量证据及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一单类别为 Qoder。

验证收口：schema 和历史输入双键反例在旧实现先红，修复后定向 3/3 通过；新增单别名正例 1/1 通过。完整 Rust 独立串行为 library 3502 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Kiro 私有离线 receipt Node、全量后端 Node audit/smoke、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 核对无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。八个 auth-kind×region `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.245 Qoder 显式非字符串语义文本不得被吞掉

2026-10-12，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter，但其 `AGENTS.md` 要求先使用本会话不可用的 `project-doc` 技能；读取门禁后未读取业务源码，也未运行访问该仓源码的差分审计，最新增量继续 `unreviewed_skill_gate`。本项仅审目标 Server 的 Chat/内容块文本合同和冻结的自包含 CLI oracle，不推断新 COSY wire。目标 decoder 原把显式非字符串的 reasoning 别名、`content_block_delta.data.text` 和 `type=text` 内容块 `text` 当成缺失，随后可由成功终态洗成空回答；固定反例在旧实现先红。现这些字段只接受缺失、null 或字符串，三个 reasoning 别名即使被首选字段遮住也逐一验证；其他显式类型在转发前返回本地 502 并锁存失败。合法字符串和 null 的兼容形状保持。Global OAuth、CN OAuth、Global PAT × Claude/Codex/Gemini × 流/非流十八组回环均确认只向原绑定账号发一次生成请求、不触发 refresh。

此项仅修本地响应语义，不改 COSY endpoint/header/body/签名、Global/CN 站点和 OAuth/PAT rail、Account/Provider/Share 单绑定、唯一终态+EOF、共享 attempt/401 预算或 receipt schema v2/revision 2。离线畸形文本不证明厂商实际发送；Global OAuth、Global PAT、CN OAuth 三份真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一单类别为 CodeBuddy。

验证收口：显式非字符串 reasoning 反例在旧 decoder 上先红，修复后 reasoning 三别名、内容块文本、`content_block_delta` 文本五类拒绝及 null/字符串正例通过；十八组三 rail × 三 Surface × 流/非流 HTTP 回环均在成功提交前返回 502，只向原绑定账号发一次生成请求且不触发 refresh。完整 Rust 独立串行为 library 3504 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Qoder 私有 receipt 与 CLI oracle Node 290/290、全量后端 Node audit/smoke 1909/1909、自包含 Qoder reference-delta、冻结 CLI oracle、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。未运行受 `project-doc` 门禁约束的 TokenRouter 业务源码 `--check-sources`，完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。Global OAuth、Global PAT、CN OAuth 的 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.246 CodeBuddy 空格文本分片不得被当成空心跳

2026-10-12，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 WorkBuddy `sse.go::sanitizeDelta/dropEmptyString`：只移除恰好 `""` 的稠密文本字段，保留 `" "`；推荐仓未跟踪 `proxy.html`、`proxy.md` 排除。目标 sanitizer 原对 `content`、`reasoning_content`、`reasoning` 和 `refusal` 调用会裁剪字符串的 `semantic_empty`，独立空格分片被吞，`"left" + " " + "right"` 可能变成 `"leftright"`。decoder→非流聚合的固定反例在旧实现先红。现这些文本字段只删除 null 或真正的空字符串，原样保留空格；显式非字符串值在流式转发前本地拒绝，与非流聚合器的字符串边界一致。Intl/CN × Claude/Codex/Gemini × 流/非流十二组回环确认空格片段到达下游，只向原绑定账号发送一次生成请求且不触发 refresh。

此项仅修本地响应文本保真，不改 WorkBuddy endpoint/header/body、Intl/CN 站点、Account/Provider/Share 单绑定、唯一 `[DONE]`+EOF、同账号恢复预算、receipt schema v2/revision 2 或 CB-N4 disabled。离线空格帧不证明厂商实际发送；Intl/CN 两份真实 receipt、CN `deepseek-v4.1-flash` 目录/接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一单类别为 Antigravity。

验证收口：空格独立分片的 decoder→非流聚合反例在旧实现先红，修复后内容与 reasoning 精确拼接、四类显式非字符串文本拒绝通过；新增 Intl/CN × Claude/Codex/Gemini × 流/非流十二组 HTTP 回环证明空格到达下游，且仅向原绑定账号发一次生成请求、不触发 refresh。首次测试编译因新增测试误用不存在的 `push/finish` 便捷方法失败，改用现有 `push_classified/finish_classified` 后生产反例才正式先红。完整 Rust 独立串行为 library 3507 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit/smoke 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。Intl/CN 的 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.247 Antigravity Gemini 末尾纯 thought 历史 turn 清理

2026-10-12，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`。前者新增 `a2718314` 在 Gemini 目标删除末尾纯 thought 的 model turn，以规避只追加空 user turn 仍被拒绝的历史；另一个 `15d2148b` 针对 model-role 嵌套媒体工具结果，目标转换通常使用 user-role，未据此强行迁移。参考不证明 Google 对目标请求的实际接受性。

目标 Codex Responses 的有效签名 reasoning 项，经 Responses→Anthropic→Gemini 的真实转换链，可成为末尾只有 thought part 的 `role=model` turn；首次 Antigravity 出站与 replay 规范化原都未清理。固定全链路反例在旧实现先红，显示最终 v1internal envelope 保留该尾部。现仅当最终模型明确为 Gemini 时，在最终 wire 边界连续删除这样的尾部 turn，并至少保留一个 turn；正文、函数调用、媒体、未知 part 字段、Claude 目标及无模型请求保留。首次封装和 replay envelope 共用幂等处理，不用合成空 user turn 掩盖错误。仅修本地历史边界，不改 endpoint/header、AG-N6 requestType、AG-D3 replay/compaction、Account/Provider/Share 单绑定或 receipt schema v2/revision 2。离线反例不等于厂商真实接受；双 OAuth rail 真实 receipt、AG-N6/AG-D3 接受性和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一单类别为 Claude。

验证收口：真实 Responses→Gemini→v1internal 反例旧实现先红、修复后通过；wire 回归覆盖连续尾部、幂等、至少保留一 turn、正文/工具/媒体保护、外层最终模型优先于内层客户端模型及 raw body 不提前清理。第一版正例断言误把既有工具 ID 规范化视为本项改动，修正为检查 turn/part 保留后通过。完整 Rust 独立串行为 library 3509 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 Node 165/165、全量后端 Node audit/smoke 1909/1909、冻结 Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、本地服务 smoke 与差分空白检查通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。两条 `RUN_REAL=0` rail 各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.248 Claude 无效缓存标记不得挤掉有效断点

2026-10-12，按循环只处理 Claude。只读复核 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 已提交 `be8c35f2`：`isValidClaudeCacheControl` 只将 `type=ephemeral` 的对象纳入缓存断点决策，null、非对象与其他 type 不占四项限额，也不改变 TTL 顺序或阻止默认注入。参考并未证明厂商接受任意无效缓存字段；目标 Native CLI 原始输入也不因本项被静默删除。

目标 `collect_cache_paths` 原只检查字段是否存在，null/布尔/其他 type 会挤占四断点预算，并让其后有效的 `ttl=1h` 被误降级；最近 user 块上的 null 还会阻止默认滚动断点注入。固定反例在旧实现先红。现只把 `type=ephemeral` 对象当成策略断点，无效字段不参与计数、TTL 和默认 system/tool/message 注入判定；默认注入可覆盖命中的无效字段，其他原始字段保留给上游判断。仅修 Claude OAuth 本地缓存策略，不改 endpoint/header/CCH、Account/Provider/Share 单绑定、CL-N7/CL-N8、模型目录或 receipt schema v1/revision 1。离线反例不证明真实 Anthropic 接受性；五份真实 operation receipt、thread/beta 厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一单类别为 Codex。

验证收口：两条固定反例旧实现先红，修复后默认 system/tool/message 注入、四项限额和 TTL 顺序均通过。第一次修复后的 TTL 测试仍失败，查明夹具中一个真实五分钟工具断点按工具→系统顺序本就应降级后续一小时断点；将该工具断点改为一小时后验证目标边界，生产逻辑未为错误断言改动。完整 Rust 独立串行为 library 3511 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 与全量后端 Node audit/smoke 均通过；冻结 Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 无需改动。第一次本地 smoke 因 `cargo run` 重编译超过脚本约 30 秒的健康等待窗口而超时，同版本预构建后复跑通过。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。五个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.249 Codex OAuth Chat 工具调用 ID 长度上限与配对

2026-10-12，按循环只处理 Codex。只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 与 `codex2api@3065654914f5806af1db99ab2212cabe87cb8328`。前者 `b0debb35` 明确把 Chat 兼容转换里超过 64 字节的工具 `call_id` 缩短为前缀加 SHA-256 短摘要，冲突时重试并保留原 ID 到结果的关联；后者的原生 WS 续链接口按调用 ID 配对，但本轮未找到其对 Chat 64 字节边界的同等处理，不把它当作证明。参考不证明当前厂商账号必然接受缩短形状。

目标 Codex OAuth 的 Chat→Responses 入口原把显式过长 `tool_calls[].id` 与相应工具结果 ID 原样发往上游；实际适配器固定反例旧实现先红。现在仅在 Codex OAuth Chat 兼容入口复制原请求后，用请求内映射对过长 ID 作 UTF-8 安全前缀加 64-bit SHA-256 摘要的确定性缩短，并先预留原有短 ID、对冲突换摘要；工具调用及结果使用同一新 ID，长度不超过 64 字节。多字节与短 ID 冲突回归证明重复转换结果确定，普通 Chat→Responses 和原生 Responses 输入不变。只修本地出站兼容转换，不改 Images/WS、Account/Provider/Share 单绑定、previous-response 上下文门禁、CX-N13/CX-D3、WS prewarm disabled 或 receipt schema v1/revision 1。离线反例不证明真实 OpenAI 上游接受性；Astra、三个 Image 2.5 与 WS prewarm 五份真实 receipt、厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一单类别为 Cursor。

验证收口：Codex OAuth 实际适配器的超长工具 ID 反例在旧实现先红，新实现同一历史调用/结果精确配对且各自不超过 64 字节；多字节、冲突短 ID、重复转换确定性及普通非 OAuth 桥保真回归通过。完整 Rust 独立串行为 library 3513 passed / 1 ignored，其他目标 124 + 1 + 1 passed（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205、全量后端 Node audit/smoke 1909/1909、冻结 Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、同版本预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。五个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.250 Cursor 显式图片不得静默退化为纯文本

2026-10-13，按循环只处理 Cursor。只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/utils/cursorImages.ts`：不合法的显式图像来源抛出脱敏错误，远程图像另设 URL/地址、大小和格式限制；其未提交 Codex 工作树内容不作证据。参考不证明 Cursor 厂商接受任意具体错误文案，也不提供目标项目的大图压缩实现。

目标 `request_builder` 原在 OpenAI Chat、Responses 和 Gemini 的非法 URL、Anthropic/Gemini 的无效内联 base64 上把图像直接丢弃，仍将文字请求送入 Cursor。四协议固定反例的第一例在旧实现先红；现于请求分解前检查显式图片能否被本地提取，缺字段、无效来源及会被忽略的位置固定、脱敏地拒绝。正常 HTTP(S)、data URI 与内联图像仍进入既有加载器，由其执行内容签名、大小、远程 SSRF 与重定向检查。仅修本地“明确给图不能暗中丢图”的边界，不改每图 1 MiB 限额、不声称实现大图压缩，也不改 Cursor AgentService protobuf、OAuth/API Key rail、Account/Provider/Share 单绑定、模型目录、会话续接或 receipt schema v2/revision 2。离线反例不是两条真实账号 rail 的接受性证明；OAuth/API Key 真实 receipt 和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一单类别为 Grok。

验证收口：四协议的无效图片 URL、无效 base64、缺来源和会被忽略的位置，以及合法 HTTP(S)/data URI/内联输入的两组定向测试通过；首个旧实现反例已先红。完整 Rust 独立串行为 library 3515 passed / 1 ignored，API 合同 124/124，另两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）；Cursor 私有 Node 95/95、全量后端 Node audit/smoke 1909/1909、冻结 Cursor reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy 和差分空白检查通过，生成 coverage 无需改动。首次本地 smoke 在不同于预构建的 Cargo 增量环境下未赶上健康等待窗口；使用同一环境复跑通过，未把首轮超时归因为代码缺陷或证明其唯一成因。完整 `scripts/static-checks.sh` 会运行 Web UI 测试，按禁令未运行。OAuth/API Key 两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.251 Grok 图片编辑 multipart 字段名不能被伪装

2026-10-13，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 首选且工作树干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 图片路径，以及次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 `backend/internal/service/grok_media.go`：后者用标准 multipart reader 的 `FormName()` 读取字段名；六项未提交 Codex 工作树内容全部排除。参考不证明 direct-XAI 的新 wire 或当前厂商账号接受性。

目标 `multipart_disposition_param` 原按每个分号切开，且不检查 disposition 类型。实际图片编辑入口中 `attachment; name="image"` 被当成有效图片，固定反例旧实现先红；`form-data; name="image;ignored"` 也可能截成 `image`，重复 `name` 存在歧义，合法 quoted-pair 转义反而漏识别。现按引号外分号分段，仅承认 `form-data`，解码引号内转义，重复目标 `name` 与不完整参数不再形成有效字段。图片内容、boundary、MIME/签名/大小/数量限制和出站 JSON 不变；不改 Account/Provider/Share 单绑定、Responses HTTP/SSE/WS、GR-N4 版本/header、remote compaction disabled 或 receipt schema v1/revision 1。离线 multipart 反例不证明 xAI 实际接受性；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号证据和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一单类别为 Kiro。

验证收口：实际图片编辑入口的 `attachment` 伪字段反例在旧实现先红，修复后连同引号内分号、重复 `name` 和合法转义字段名回归通过；multipart 图片编辑专项 6/6。完整 Rust 独立串行回归为 library 3516 passed / 1 ignored，API 合同 124/124，其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 三 operation 私有 Node 159/159、全量后端 Node audit/smoke 1909/1909、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 和差分空白检查通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收返回预期 `blocked_inputs`，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.252 Kiro 预处理过的图片不得在请求构造时消失

2026-10-13，按循环只处理 Kiro。只读复核 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`：其 `src/image_resize.rs` 明确以不丢图片为处理原则，Kiro 模型请求在 `src/kiro/model/requests/conversation.rs` 将图片承载于 user message 的 `images`；参考未证明 system 或 assistant 图片有原生承载能力，也不授权猜测新的上游 wire。

目标 `prepare_anthropic_images_with_memory` 原遍历并处理 system、assistant 及对象型内容中的显式图片，但后续请求构造只搬运 user 内容数组直系 image 与其中 tool_result 内容数组直系 image。实际 `prepare_kiro_request` 的 system 图片反例在旧实现先红：出站 user `images=[]` 却成功返回。现图片预处理前核对位置，无法进入 Kiro wire 的图片固定、脱敏地 400 拒绝；正常 user 与 tool_result 图片继续原路径。图片数据、尺寸、格式、去重和内存预算规则不变；不改 Kiro/Amazon Q endpoint/header、EventStream、Account/Provider/Share 与八种 auth-kind×region 单绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线反例不证明真实厂商接受性；八份真实 receipt、KI-N5 独立计量证据和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一单类别为 Qoder，其推荐参考的技能门禁仍需另行遵守。

验证收口：实际请求构造的 system 图片反例在旧实现先红；修复后 system、assistant、对象型 user 与对象型 tool_result 图片拒绝测试，连同 user/tool_result 数组直系图片正例通过，定向 4/4。完整 Rust 独立串行为 library 3518 passed / 1 ignored，API 合同 124/124，其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）；Kiro 私有 receipt Node 与全量后端 Node audit/smoke 1909/1909、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。八个 auth-kind×region `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.253 Qoder 工具调用兼容别名不得静默改写身份

2026-10-13，按循环只处理 Qoder。`Qoder/AGENTS.md` 推荐 TokenRouter；其 `AGENTS.md` 要求先使用本会话未加载的 `project-doc` 技能。读取门禁后没有读取推荐仓业务源码或运行其 `--check-sources`，最新参考业务增量继续保持 `unreviewed_skill_gate`。本项只依据目标 Server 的工具分片身份合同、冻结自包含 CLI oracle 和本地回环，不推断新 COSY wire。

目标把 `id`、`tool_call_id`、`call_id` 和 `function.name`、`name`、`tool_name` 分别当作同一工具 ID/名称的别名，却只采用首个非空值。`id=call-a`、`call_id=call-b` 的固定反例在旧实现先红，错误地向下游输出 `call-a`。现并存的非空别名必须一致，显式非字符串/null 值也不能被回退掩盖；冲突在转发前固定、脱敏地返回本地 502。相同别名、单一别名及工具 ID 的原始空白保留；不改 COSY endpoint/header/body、Global/CN 站点、OAuth/PAT rail、Account/Provider/Share 单绑定、唯一终态+EOF、共享 attempt/401 预算或 receipt schema v2/revision 2。离线别名冲突不能证明厂商真实发送；三条 rail 真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一单类别为 CodeBuddy。

验证收口：冲突 ID 首例在旧实现先红，修复后连同冲突工具名、非法 ID 类型及相同别名/原始空白正例通过；Global OAuth、CN OAuth、Global PAT × Claude/Codex/Gemini 九组 HTTP 回环均在响应提交前返回 502，仅向原绑定账号发一次生成请求且不触发 refresh。Qoder 定向 Rust 86/86；完整 Rust 独立串行为 library 3520 passed / 1 ignored，API 合同 124/124，其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Qoder 私有 receipt 与 CLI oracle Node 290/290、全量后端 Node audit/smoke 1909/1909、自包含 Qoder reference-delta、冻结 CLI oracle、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。没有运行受 `project-doc` 门禁约束的 TokenRouter 业务源码 `--check-sources`，完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。三条 `RUN_REAL=0` rail 各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.254 CodeBuddy 显式非数组工具分片不得在流式清洗时消失

2026-10-13，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐且业务工作树干净的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 WorkBuddy SSE：非流聚合按工具数组读取，流式清洗可删除空占位。参考的宽松错误处理不定义本 Server 的成功门槛；未跟踪的 `proxy.html`、`proxy.md` 全部排除。

目标非流聚合器已拒绝非数组 `tool_calls`，流式清洗器却只在字段是数组时校验；显式空字符串会被删除，随后 `finish_reason=tool_calls` 可被正常转发。实际解码入口固定反例在旧实现先红。现显式字段只允许数组或 `null`，其余类型在响应业务转发前固定、脱敏地 502 拒绝；空占位、合法工具分片和无 index 的既有兼容不变。此项不改 CodeBuddy 出站 endpoint/header/body、Intl/CN 站点、Account/Provider/Share 单绑定、唯一 `[DONE]`+EOF、同账号 401 预算或 receipt schema v2/revision 2。离线形状不能证明厂商真实发送；两站真实 receipt、CN 精确模型能力证据与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一单类别为 Antigravity。

验证收口：实际流式解码的空字符串 `tool_calls` 反例旧实现先红；修复后空字符串、对象、数字、布尔值拒绝及 `null` 占位正例通过。Intl/CN × Claude/Codex/Gemini × 流/非流十二组 HTTP 回环均在成功提交前返回 502，仅向原绑定账号发一次生成请求且不触发 refresh。CodeBuddy 定向 Rust 53/53；完整 Rust 独立串行为 library 3522 passed / 1 ignored，API 合同 124/124，其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit/smoke 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy、预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行。Intl/CN 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.255 Antigravity 显式非字符串工具 ID 不得被自动生成或配对掩盖

2026-10-13，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `Antigravity-Manager@601f5e7375343343922ccab5e50653bc3450c3b` 已提交工具回放实现：工具调用/结果用字符串 ID 追踪；缺失 ID 可由各自局部规则处理。参考并不证明厂商接受显式数字/null ID，也不授权放宽本 Server 的最终出站身份边界。

目标在 `normalize_tool_call_ids` 中将显式非字符串 `functionCall.id` 当作缺失并生成 `tool_call_0`，也可把非字符串 `functionResponse.id` 当缺失后按唯一工具名配对；数字调用 ID 的固定反例在旧实现先红。现最终 wire 规范化前同时核对 camelCase/snake_case 工具调用和结果，显式 ID 必须是字符串，错误类型固定、脱敏地本地 400 拒绝；真正缺失 ID 的原兼容生成与配对保持。仅修本地出站身份，不改 Antigravity/AGY endpoint/header/body 其他字段、模型/requestType、签名 replay、Account/Provider/Share 单绑定、同账号恢复或 receipt schema v2。离线类型反例不能证明厂商真实发送；双 rail 真实 receipt、AG-N6、compaction 与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一单类别为 Claude。

验证收口：显式数字调用 ID 的固定反例先红后绿；数字结果 ID、孤立结果的 `null` ID、snake_case 布尔 ID 均拒绝，真正缺失 ID 的配对仍通过。Antigravity 定向 Rust 97/97；完整 Rust 独立串行回归为 library 3523 passed / 1 ignored，API 合同 124/124，其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 Node 165/165、全量后端 Node audit/smoke 1909/1909、冻结 Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。`antigravity_oauth`、`agy_oauth` 两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.256 Claude redacted thinking 不透明内容块不得被误判为普通思考

2026-10-13，按循环只处理 Claude。只读核对 `Claude/AGENTS.md` 推荐且业务工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 Responses 转换 fixture：`redacted_thinking` 由带非空字符串 `data` 的独立 `content_block_start` 表示，直接 `content_block_stop`，不接受普通 thinking 文本增量。参考的宽松 executor 完成检查不定义目标 Server 的严格成功边界；此形状仍需真实 Anthropic 接受性验收。

目标 SSE 检查器原将 redacted 和普通 thinking 混为同一类别，缺失、空值或非字符串 `data` 可进入正常成功流，`thinking_delta` 也可附着其上；非流式 Messages JSON 同样会接纳缺 `data` 的 redacted 块。两条固定反例在旧实现先红。现将 redacted 作为独立块类别，要求非空字符串 opaque `data`，拒绝后续 delta；有效数据不解析、不改写、不记录，未知未来块仍按原兼容策略处理。仅修 Claude 入站响应完整性，不改出站 Messages/count_tokens、CL-N7/CL-N8、Account/Provider/Share 单绑定、模型目录或 receipt schema v1/revision 1。离线反例不能证明厂商真实发送；五项真实 operation receipt、thread/beta 接受性和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Codex。

验证收口：SSE/JSON 两条反例旧实现均先红，修复后缺失、`null`、数字、空字符串 `data` 与 redacted 后的 thinking delta 均拒绝；有效不透明块和原有未知未来块保持通过。Claude HTTP 非流入口对缺 `data` 返回 502。完整 Rust 独立串行回归为 library 3525 passed / 1 ignored，API 合同 124/124，其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 252/252、全量后端 Node audit/smoke 1909/1909、冻结 Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 各为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.257 Codex WebSocket 已完成响应的迟到中断不得污染下一轮

2026-10-13，按循环只处理 Codex。只读复核 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233`。前者 `6331b38b` 支持原始 `response.interrupt` 控制帧，`d318bcc3` 修复完成后的迟到中断污染下一轮；后者最新定价数据库锁提交不作为中断协议依据。参考不证明目标私有 OAuth entitlement，也不要求复制其 HTTP 本地取消接口。

目标保留的上游 WebSocket 原会收到已完成响应的迟到中断，本地真实回环旧实现先红。现只对本桥接、同一物理上游连接已观测终态的响应 ID 作有界记录并幂等忽略；未知空闲中断返回脱敏本地错误，活动中断保留原始帧及扩展字段，写入待完成时也走有界队列。终态记录最多 32 项、单 ID 最多 512 字节，连接变化隔离。仅修 Codex WS 控制帧生命周期，不改出站 `response.create`、Images、Account/Provider/Share 单绑定、同账号恢复预算、WS prewarm disabled 或 receipt schema v1/revision 1。HTTP fallback 期间仍不支持从 WS 数据帧触发本地 HTTP 取消；未观测终态或记录已淘汰时也不承诺幂等。五份真实 operation receipt 与当前工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Cursor。

验证收口：迟到中断真实 WebSocket 回环旧实现先红，修复后连续两轮无上游污染；活动中断回环保留原始控制帧，ID 类型、终态记录容量/连接隔离和 pending-write 控制队列单测通过。冻结 Codex reference-delta 首次因旧 steering 测试锚点被改名而失败，将新增中断断言拆为独立测试、保留原锚点后通过，没有放宽审计。完整 Rust 独立串行为 library 3529 passed / 1 ignored、API 合同 124/124、其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）；Codex 私有 Node 212/212、全量后端 Node audit/smoke 1909/1909、Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.258 Cursor 大图必须先安全缩放再适配 1 MiB wire

2026-10-13，按循环只处理 Cursor。只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/utils/cursorImages.ts`：单图可先读取到 16 MiB，再经尺寸/像素防炸弹、JPEG 质量和边长阶梯处理到不超过 1 MiB，默认/高细节有不同软目标；最多 12 图。其未提交 Codex 工作树改动不作证据。参考不证明本 Server OAuth/API Key 的真实账号接受性。

目标原在 Cursor 图片 loader 上把 1 MiB wire 限额当入口上限，约 3 MiB 的真实有效 PNG 在旧实现固定反例先红。现分离入口和最终 wire 上限，保留既有签名、MIME、远程 SSRF 与重定向核验，新增解码前最长边/总像素门槛、阻塞线程 JPEG 质量/边长处理、请求内存预留以及 Chat/Responses `detail=high/original` 质量目标；输出仍不得超过 1 MiB，过大、损坏或不支持的图片固定脱敏地失败，不降为纯文本。仅改 Cursor AgentService 的图片准备，不改 protobuf 字段、Account/Provider/Share 单绑定、OAuth/API Key 选择、模型目录、会话续接、同账号恢复、CUR-N1 两条私有 receipt 门禁或 schema v2/revision 2。真实 receipt 与当前未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。本轮验证收口后下一类别为 Grok。

验证收口：有效约 3 MiB PNG 在旧 loader 先因 1 MiB 入口限额红灯，新路径成功产出 JPEG 并保持最终 1 MiB 上限；默认和高细节软目标、尺寸/像素炸弹、损坏图片、请求内存耗尽及 12 图前置门槛有定向测试。首次新增高细节参数后的编译仅因两处旧测试调用少传参数失败，补齐后 Cursor 模块 Rust 273/273 通过。完整 Rust 独立串行为 library 3535 passed / 1 ignored、API 合同 124/124、其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）；Cursor 私有 Node 95/95、全量后端 Node audit/smoke 1909/1909、冻结 Cursor reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 和差分空白检查均通过，生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。OAuth/API Key 两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.259 Grok 工具选择不能脱离有效工具声明

2026-10-13，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 推荐且干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`；其 `df4dde39` 与 `6db9f67f` 已提交实现/回归要求 `tool_choice` 只和非空有效工具数组一起到达 Grok Console。次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 文件仍排除。参考不证明本 Server 的 Grok OAuth 真实账号接受性。

目标 Grok `sanitize_tools` 原在 `tools` 缺失时直接返回，导致孤立 `tool_choice` 留在实际出站请求；`tools=null` 更早被误当畸形数组 422 拒绝，空数组已有清理。HTTP 首个固定反例旧实现先红。现把缺失、`null` 与空数组统一成无工具状态，两字段一起省略；非 null 畸形类型仍 422，非空有效工具和原有匹配选择保留，HTTP/WS 共用。只修最终 Grok 工具配对边界，不引入默认工具、不重排 Account/Provider/Share、模型、reasoning、同账号恢复或 receipt schema v1/revision 1。GR-N2 inference/media/remote_compaction 三份真实 receipt 及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Kiro。

验证收口：缺失工具时孤立 `tool_choice` 的实际 HTTP 请求反例在旧实现先红；修复后缺失、`null`、空数组在 HTTP/WS 均删除两字段，合法 `web_search` + `auto` 保留，非 null 畸形工具类型仍 422。定向 Rust 1/1；完整 Rust 独立串行为 library 3536 passed / 1 ignored、API 合同 124/124、其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 私有 receipt Node 159/159、全量后端 Node audit/smoke 1909/1909、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。inference、media、remote_compaction 三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.260 Kiro reasoning 显式畸形字段不得静默丢失

2026-10-13，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐且干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `ReasoningContentEvent`：`text`、`signature`、`redactedContent` 为可选字符串，显式数字/对象/数组不是有效字段。参考未定义本 Server 既有包裹或别名兼容，也不能证明厂商实际发送的全部形状。

目标原将 reasoning 事件收成任意 JSON，后续 `as_str()` 读取会把显式错误类型当作字段缺失；对象型 `redactedContent` 的固定 EventStream 反例在旧实现先红，却可让流走到正常结束。现共用解析入口对目标已识别的文本、签名和加密思考字段要求字符串或 `null`；顶层或包裹的 reasoning 只接受对象、字符串和既有空占位，未知对象字段继续兼容。畸形值以固定脱敏 `KIRO_EVENT_STREAM_INVALID` 拒绝，非流与流式共用，流式不发成功 `message_stop`；合法 opaque 字符串保持原样。仅修入站成功完整性，不改 Kiro/Amazon Q 出站请求、八种 auth-kind×region 的 Account/Provider/Share 单绑定、EventStream CRC/容量/EOF、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。八份真实 receipt、KI-N5 专项证据和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一单类别为 Qoder。

验证收口：EventStream 解析入口的对象型 `redactedContent` 首例旧实现先红；修复后数字文本、数组签名、畸形包裹值均拒绝，字符串/`null` 正例通过；非流与流式闭环都返回脱敏错误，流式不发成功终态。reasoning 关键词 Rust 72/72；完整 Rust 独立串行为 library 3537 passed / 1 ignored、API 合同 124/124、其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Kiro 私有 receipt Node 492/492、全量后端 Node audit/smoke 1909/1909、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。八个 auth-kind×region `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.261 Qoder 空心跳不能靠 `[DONE]` 变成成功回答

2026-10-13，按循环只处理 Qoder。首选 TokenRouter 的 `project-doc` 技能当前不可调用，遵守其门禁，不读业务源码也不运行其 `--check-sources`；仅借目标 Server 的官方 CLI oracle 和次选 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `9d5eb0b` 做差分。后者在首次下游字节前拒绝仅心跳的 200 流，非流也拒绝空回答；未跟踪 `proxy.html`、`proxy.md` 排除。其账号池 failover 与任意截断流补终态不迁入 Server。

目标原把裸 `[DONE]` 当成功终态，空 wrapper/空 delta 后的 EOF 会附加成功 `[DONE]`；非流聚合器甚至能把仅 `[DONE]` 补为 `stop`。固定空流反例旧实现先红。现共用解码器仅转发有意义的规范 chunk，完成时要求有效输出、明确结束理由、唯一终态和真实 EOF；已有内容的 Anthropic 风格 `message_stop` 可按是否有工具生成明确 `stop`/`tool_calls`，原生 Chat 仅有内容加裸 `[DONE]` 仍失败关闭。非流直接聚合的空 `[DONE]` 同样拒绝。三凭据 rail × 三 App × 流/非流本地回环均在响应提交前 502，仅向原绑定账号发送一次生成请求且不 refresh。只修入站成功边界，不改 COSY 出站、站点/凭据 rail、Account/Provider/Share 单绑定、同账号 401 预算或 receipt schema v2/revision 2。真实三 rail receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 CodeBuddy。

验证收口：裸 `[DONE]` 空流首例旧实现先红；修复后空 wrapper/空 delta、非流直接聚合及“有文本但没有结束理由”均拒绝，已有内容/工具的 Anthropic `message_stop` 与原生明确 finish 正例保留。首次绿灯编译仅因新增 HTTP fixture 把 `Bytes` 直接当字符串格式化而失败，改为字节拼接后通过，未改变生产逻辑。三 rail × 三 App × 流/非流 18 组本地回环均在提交前 502、单绑定账号生成一次且零 refresh；Qoder 关键词 Rust 105/105。完整 Rust 独立串行为 library 3540 passed / 1 ignored、API 合同 124/124、其他两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Qoder 私有 receipt Node 281/281、全量后端 Node audit/smoke 1909/1909、自包含 Qoder reference-delta 与官方 CLI oracle、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。按 TokenRouter 的 `project-doc` 门禁未读其业务源码，也未运行读取外部对象的 `--check-sources`；完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行，没有做部署测试。三条 `RUN_REAL=0` rail 各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.262 CodeBuddy usage-only 尾帧的流/非流兼容

2026-10-13，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/workbuddy/sse.go::Aggregate`：`Choices` 可缺省而 `Usage` 仍保留；两个未跟踪文档排除。目标流式 decoder 原已接受只有对象 `usage` 的独立尾帧，非流聚合器却要求每个 JSON 帧都有 `choices`。固定 decoder→aggregator 反例在旧实现先以 502 红灯。现仅在 usage 已通过对象形状校验时允许缺省 choices，显式非数组 choices、缺两者、畸形 usage 继续失败；缓存 read/write、total、唯一 `[DONE]` + EOF 保持。Intl/CN × Claude/Codex/Gemini × stream/non-stream 十二组离线 HTTP 回环已验证同一尾帧均可返回缓存计量，单绑定账号仅生成一次、零 refresh。参考宽松的首个 `[DONE]` 停止扫描不迁入目标；不改 WorkBuddy 出站 wire、Account/Provider/Share、同账号 401 预算、receipt schema v2/revision 2 或 CB-N4 disabled。离线省略形状不证明厂商实际发送；双站真实 receipt 和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一类别为 Antigravity。

验证收口：固定单元反例旧实现先红，修复后 CodeBuddy 关键词 Rust 105/105，缺省/畸形 choice 与 usage 负例维持拒绝。完整 Rust 独立串行为 library 3543 passed / 1 ignored、API 合同 124/124、其余两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit/smoke、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。Intl/CN 两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.263 Antigravity 末尾模型角色媒体工具结果的 turn 修复

2026-10-13，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 `15d2148b`：其 Gemini 上游 HTTP fixture 对末尾 `model` 角色、`functionResponse.parts` 含图片媒体报 400，并用追加空 `user` turn 修复；文本工具结果与 user 角色媒体结果不变。另一推荐 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 仅作预算/签名背景，不将其桌面或池化策略迁入。本轮还核对 CLIProxyAPI `9126772d` 的 schema dependent 关键词：目标 Antigravity 已有 allowlist 删除，故不重复修改。

目标原生 Gemini 输入可以保留模型角色的媒体工具结果，最终封装的固定反例在旧实现先红。现仅当最终外层模型明确为 Gemini，且末尾 `model`/`assistant` 角色工具结果的嵌套 parts 含 `inlineData`/`fileData`（含 snake_case）时追加一次空 user turn。已验证幂等，普通文本结果、原 user 角色媒体结果、Claude 最终模型及无 envelope 的 raw body 原样保持；图片字节和工具 ID/名称关联不改。Antigravity OAuth 与 AGY OAuth × 原生 Gemini 适配器的最终请求回归通过。只修最终入站历史转换，不改上游 endpoint/header/其余 body、AG-N6 requestType、AG-D3 replay/compaction、Account/Provider/Share 单绑定、同账号恢复或 receipt schema v2/revision 2。离线反例不能证明 Google 真实接受；两条 rail 真实 receipt、AG-N6/AG-D3 专项证据与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一类别为 Claude。

验证收口：最终 wire 固定反例旧实现先红，修复后 Antigravity 关键词 Rust 110/110，双 rail 原生 Gemini 封装与正反例通过。完整 Rust 独立串行为 library 3545 passed / 1 ignored、API 合同 124/124、其余两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt Node 165/165、全量后端 Node audit/smoke、冻结 Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。`antigravity_oauth`、`agy_oauth` 两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.264 Claude 历史工具 ID 不得以不可移植字符出站

2026-10-13，按循环只处理 Claude。只读核对 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 `e9c8769a`：Claude executor 将历史 `tool_use.id` 与 `tool_result.tool_use_id` 中非 `[A-Za-z0-9_-]` 的非空 ID 成对改成可移植别名，并避免与原有合法 ID 碰撞；其 Devin 专属双向编码不迁入本 Server。目标 Claude OAuth 最终请求原只处理工具名称，`call/one` 固定反例在旧实现原样出站、先红。现于最终 CCH 签名前用域分离 SHA-256 URL-safe 短别名做请求内 ID 映射，调用/结果共用，预占合法 ID，碰撞顺延；合法 ID、普通字段、空值和非字符串不因本项改变，二次规范化幂等。Claude OAuth 生产请求准备入口与 Messages/count_tokens 共用处理；不改 endpoint/header/beta、工具名恢复、CL-N7/CL-N8、Account/Provider/Share 单绑定、模型目录或 receipt schema v1/revision 1。离线反例不能证明厂商真实拒绝的完整形状；五份真实 operation receipt、thread/beta/工具 ID 接受性和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一类别为 Codex。

验证收口：固定 OAuth 请求反例旧实现先红，修复后 Claude OAuth 关键词 Rust 86/86，合法 ID 占位碰撞、调用/结果一致、普通字段保留、幂等及 Provider 生产请求准备入口通过。完整 Rust 独立串行为 library 3547 passed / 1 ignored、API 合同 124/124、其余两目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 252/252、全量后端 Node audit/smoke、冻结 Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.265 Codex 无工具请求不得保留并行工具调用标志

2026-10-13，按循环只处理 Codex。只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 和 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233`。前者已提交 `75fa6265` 在最终 `tools` 缺失或为空时删除 `parallel_tool_calls`，非空工具保持原设置；后者近期原生 WS 续链与共享上下文改动不作为这个字段的独立协议证明，也不迁移其连接池策略。目标 Codex OAuth 普通路径原在无工具时补 `tools: []` 与 `parallel_tool_calls: false`，客户端显式 `true` 可原样出站；Lite 无条件写入 `false`，Compact/WS 也缺少同一最终工具配对门槛。普通/Compact/Lite 实际请求准备的固定反例旧实现先红。现工具清洗后共用判断，最终顶层工具数组缺失或为空时省略并行标志；有工具时普通/Compact/WS 保持既有值，Lite 仍按原规则强制 `false`。普通 Responses、Compact、Lite、原生 WS 与 WS→HTTP fallback 共用这些准备边界；不改 endpoint/header、模型、工具 schema、Images、Account/Provider/Share 单绑定、同账号恢复、WS prewarm disabled 或 receipt schema v1/revision 1。离线反例不证明厂商真实接受；五份真实 operation receipt 和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Cursor。

验证收口：普通/Compact/Lite 固定反例在旧实现先红，修复后有工具正例、图片工具剔除后为空及原生 WS 普通/Lite 分支通过。完整 Rust 独立串行为 library 3550 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1、主程序测试目标 0/0（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205、全量后端 Node audit/smoke、冻结 Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.266 Cursor 内建工具桥接不得靠名称子串猜测

2026-10-13，按循环只处理 Cursor。只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/executors/cursor/builtinToolBridge.ts`；该文件的工作树对象与 HEAD 一致，22 项未提交 Codex 文件排除。参考 `namedTools` 只按显式完整别名选原生工具候选，随后用声明 schema 核对，不会把 `undelete` 猜作 Delete。目标 `resolve_mcp_tool_by_aliases` 原还允许 `lower.contains(alias)`，导致原生 Delete/Read/Grep/Shell 可误选包含相应片段但语义不同的客户端工具；生产桥接 helper 的 `undelete` 固定反例旧实现先红。现匹配只接受大小写无关的完整固定别名，或目标 Responses 点号命名空间的完整末段；`functions.grep` 保持，`functions.undelete`、`breadboard`、`research_notes`、`unshell` 不再误选。合法 `read_file`、`web_fetch`、`ripgrep` 仍由既有显式别名和 schema 验证处理；未匹配的内建工具走原有类型化上游拒绝，不输出伪工具调用。此项只修本地工具路由，不推断新 Cursor wire，`CUR-N2=reviewed_no_wire_delta` 不变；不改 AgentService protobuf、OAuth/API Key rail、Account/Provider/Share 单绑定、会话续接、模型目录、图片、同账号恢复或 receipt schema v2/revision 2。OAuth/API Key 两份真实 receipt、厂商接受性与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Grok。

验证收口：`undelete` 固定反例在旧实现先红；修复后四组误匹配负例、点号命名空间完整叶子正例和既有显式别名矩阵均通过。Cursor 定向 Rust 274/274；完整 Rust 独立串行为 library 3551 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Cursor 私有 receipt Node 95/95、全量后端 Node audit/smoke、冻结 Cursor reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。OAuth/API Key 两条 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.267 Grok 强制函数选择必须匹配已声明函数名

2026-10-13，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 首选且工作树干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `responses_tool_choice.go::normalizeToolChoice`，其对函数选择核对已声明工具身份；次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 `openai_gateway_grok.go::shouldDropGrokToolChoice` 在最终过滤后比较函数名，六处未提交 Codex 文件排除。前者完整别名转换及后者账号池/质量策略均不迁入目标。目标 `should_drop_tool_choice` 原仅比较 `type`，导致仅声明 `lookup` 时强制选择 `missing` 仍出站；实际 HTTP 请求固定反例旧实现先红。现对象型函数选择必须与清理后保留的函数名精确匹配，hosted 工具仍按类型，普通 `auto|none|required` 不变；HTTP Responses、Chat 转换、WebSocket `response.create` 和其 HTTP fallback 共用请求准备规则。不改 endpoint/header、模型、reasoning、Account/Provider/Share 单绑定、同账号恢复、quality observer 或 receipt schema v1/revision 1。离线反例不证明 xAI 真实接受；GR-N2 三份真实 receipt 和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Kiro。

验证收口：HTTP `missing` 固定反例旧实现先红，修复后已声明函数、未声明函数、嵌套函数选择和 hosted 工具在 HTTP/WS 均通过；定向 Rust 1/1。完整 Rust 独立串行为 library 3552 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 私有 receipt Node 159/159、全量后端 Node audit 1896/1896 + smoke 13/13、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。inference、media、remote_compaction 三个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.268 Kiro 无名工具调用不得以成功工具块出站

2026-10-13，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/kiro/model/events/tool_use.rs::ToolUseEvent`：工具 `name` 必填，而 `input`/`stop` 可缺省。目标为兼容分片，允许后续事件省略名称，不能直接复制参考的逐帧必填规则。目标累积器原可让同一 ID 的所有事件都无名称，最终以 `name:""` 的 `tool_use` 和成功终态输出；`stop=true` 的实际 EventStream 固定反例旧实现先红。现仅在完成调用或 EOF 恢复无参调用时要求已绑定非空白名称；前片有名、后片省略仍可正常完成，原有工具名冲突、JSON 截断/错误和无参恢复边界不变。流式/非流式共用累积器，以固定脱敏 `KIRO_EVENT_STREAM_INVALID` 拒绝无名完成。只修入站成功完整性，不改 Kiro/Amazon Q 出站请求、八种 auth-kind×region Account/Provider/Share 单绑定、EventStream CRC/容量/EOF、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线反例不证明厂商真实发送；八份真实 receipt、KI-N5 专项证据和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Qoder。

验证收口：实际 EventStream `stop=true` 无名工具反例旧实现先输出成功空名调用，修复后已停止/未停止空输入在非流式与流式 builder 均固定拒绝，后续片段省略已绑定名称的正例保留；定向 Rust 3/3。完整 Rust 独立串行为 library 3554 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Kiro 私有 receipt Node 492/492、全量后端 Node audit 1896/1896 + smoke 13/13、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。八个 auth-kind×region `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.269 Qoder 强制选择不存在的工具不得放宽为全部工具

2026-10-13，按循环只处理 Qoder。`Qoder/AGENTS.md` 首选 TokenRouter 的 `project-doc` 技能在当前会话不可调用，遵守其门禁，未读取业务源码、运行仓库命令或执行 `--check-sources`；最新参考增量维持 `unreviewed_skill_gate`。次选 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/qoder/adapter.go` 保留客户端工具列表与选择字段；两个未跟踪文档排除。该参考不能替目标决定 COSY 不转发选择字段时的失败策略，也不迁入账号池。目标实际 payload builder 原用选择名称过滤工具，但名称不存在时悄悄回退完整工具列表；Global/CN 固定反例旧实现先红。现明确区分 `none`、`auto`、`required` 和指定函数：指定函数只暴露精确匹配工具，未声明或畸形选择、无工具的 `required` 发网前拒绝，不再把强制调用退化为自动选择。Claude/Codex/Gemini 入口共用规范 Chat builder；不改 COSY wire 编码/签名、站点/凭据 rail、Account/Provider/Share 单绑定、同账号 401 预算、模型目录或 receipt schema v2/revision 2。离线反例不证明厂商真实接受；Global OAuth、Global PAT、CN OAuth 三条真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审与当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 CodeBuddy。

验证收口：Global/CN 实际 payload 的未声明函数选择旧实现先回退全部工具，修复后自动/必需/禁止/合法指定、畸形指定、未知模式和无工具必需模式定向 Rust 2/2；三 rail × Claude/Codex/Gemini 本地 HTTP 回环 9 组均在发起生成前返回 400，单绑定账号生成零次、refresh 零次。完整 Rust 独立串行为 library 3557 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Qoder 私有 receipt Node 281/281、全量后端 Node audit 1896/1896 + smoke 13/13、自包含 Qoder reference-delta 与官方 CLI oracle、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。按 TokenRouter 的 `project-doc` 门禁未读取其业务源码，也未运行外部源码的 `--check-sources`；完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行，没有做部署测试。三条 `RUN_REAL=0` rail 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.270 CodeBuddy 非空旧式 function_call 的失败关闭

2026-10-13，按循环只处理 CodeBuddy。只读复核 `CodeBuddy/AGENTS.md` 首选的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 WorkBuddy SSE：清洗器删除空 `function_call`，非流聚合器只累计 `tool_calls`；两个未跟踪文档排除。目标同样不聚合旧式调用，但原会保留非空 `delta.function_call`，使 `finish_reason=function_call` 加 `[DONE]` 在非流路径生成未包含该工具的成功回答。实际 decoder 固定反例旧实现先红。现流式 decoder 与直接非流聚合器都拒绝非 dummy 旧式字段，空占位继续清理，正常 `tool_calls` 不变。只修本地入站成功完整性，不推断新的两站 wire，不改 WorkBuddy 出站请求、双站与单账号绑定、同账号 401 预算、唯一终态+EOF 或 receipt schema v2/revision 2。Intl/CN 真实 receipt、CN 精确模型能力证据及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，未做 UI 自动化或部署测试；本轮验证收口后下一类别为 Antigravity。

验证收口：固定 decoder 反例旧实现先红、修复后通过；直接聚合入口和 Intl/CN × Claude/Codex/Gemini × 流/非流十二组 HTTP 回环均拒绝非空旧式调用，只向原账号生成一次、零 refresh。完整 Rust 独立串行为 library 3559 passed / 1 ignored、API 合同 124/124、其他两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit/smoke 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。Intl/CN 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.271 Antigravity 双拼写工具字段不得表达两个身份

2026-10-13，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐、工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 和 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`：前者 Gemini 转换识别 `functionCall`/`function_call`、`functionResponse`/`function_response` 两套写法，后者 pipeline estimator 优先选驼峰再回退下划线；参考都不能证明同一 part 同时带两套字段的真实厂商语义。目标最终 wire 原也只给优先项规范化 ID 和关联结果，另一项可带着冲突的工具身份出站。调用 ID 冲突的固定反例旧实现先红。现最终出站前拒绝同一 part 同时出现同语义双拼写，调用和结果均覆盖；即使初始值相同，归一化后仍可能分叉，因此一律拒绝。只带任一种拼写的既有兼容、工具名/ID 正规化及 replay 路径不变；错误固定且不回显工具内容。Antigravity/AGY 两 rail 的原生 Gemini 请求准备均由同一边界拒绝。不改 endpoint/header、模型/requestType、Account/Provider/Share 单绑定、AG-N6、AG-D3 replay/compaction、同账号恢复或 receipt schema v2/revision 2。离线反例不证明厂商实际发送；两条真实 receipt、AG-N6/AG-D3 厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Claude。

验证收口：最终 wire 调用 ID 冲突反例旧实现先红，修复后双拼写调用/结果及初始值相同的双拼写均拒绝，单一驼峰/下划线正例保留；Antigravity/AGY 两 rail 原生 Gemini 请求准备定向通过。完整 Rust 独立串行为 library 3561 passed / 1 ignored、API 合同 124/124、其他两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt Node 165/165、全量后端 Node audit/smoke 1909/1909、冻结 Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。`antigravity_oauth`、`agy_oauth` 两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.272 Claude 非流式已知内容块不得凭完整外壳伪装成功

2026-10-13，按循环只处理 Claude。只读核对 `Claude/AGENTS.md` 推荐且干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交原生 Messages 非流转换测试：文本块带字符串，`tool_use` 带 `id`、`name` 与对象型 `input`，转换器据此生成客户端工具调用。参考不证明畸形上游块的真实形状，也不定义本 Server 的严格成功门槛。目标 SSE 起始工具块已有较强校验，但非流 `validate_message` 原只检查外壳及 `redacted_thinking`；数字 `text.text`、对象型 `thinking.thinking`、缺 ID 或非对象 input 的 `tool_use` 可能在完整 usage 下被记为成功。JSON 检查入口固定数字文本反例旧实现先红。现对已知完整文本/思考块要求字符串，对 `tool_use`/`server_tool_use` 要求非空 ID/名称和对象 input；空文本与未知未来块保持兼容，SSE 分片仍按原生命周期规则，不套完整块校验。Claude OAuth 非流 HTTP 回环中缺 ID 工具块在业务成功前固定、脱敏地 502 拒绝，只请求绑定上游一次。不改出站 Messages/count_tokens、beta、CCH、CL-N7/CL-N8、Account/Provider/Share 单绑定或 receipt schema v1/revision 1。五项真实 operation receipt、thread/beta 厂商接受性及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Codex。

验证收口：非流 JSON 数字文本反例旧实现先红；修复后已知文本/思考/工具块的缺字段与错误类型拒绝，合法空文本及未知未来块正例保持，Claude OAuth 非流 HTTP 缺 ID 工具块固定 502，原绑定上游请求一次。完整 Rust 独立串行为 library 3563 passed / 1 ignored、API 合同 124/124、其他两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Claude 私有 receipt Node 252/252、全量后端 Node audit/smoke 1909/1909、冻结 Claude reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。
### 18.273 Codex OAuth Responses include 只转发已支持的显式来源

2026-10-13，按循环只处理 Codex。只读核对 `Codex/AGENTS.md` 推荐且干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233`；CLIProxyAPI 已提交 `2044a01f` 把 Codex 出站 `include` 限定为必需加密推理内容，以及客户端数组显式请求的网页搜索来源。目标普通 Responses HTTP 原保留未知、非字符串和重复 `include`，WS `response.create` 也没有同样边界；HTTP 固定反例旧实现先红。现两条路径共用白名单归一化，不从 `web_search` 工具声明推断来源请求，独立 Images 和 compact 不变。不改 endpoint/header、Account/Provider/Share 单绑定、同账号恢复或 receipt schema v1/revision 1。参考和离线测试均不证明厂商真实接受；五份真实 operation receipt 和未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮验证收口后下一类别为 Cursor。

验证收口：HTTP 的未知/非字符串/重复 include 固定反例旧实现先红，修复后与原生 WS 的相同来源请求结果一致；缺失、非数组和未知值回到必需项。完整 Rust 独立串行为 library 3565 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Codex 私有 receipt Node 205/205、全量后端 Node audit/smoke 1909/1909、冻结 Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；未做部署测试。五个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入与未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.274 Cursor 推荐提交态与其他分支修复隔离复核

2026-10-13，按循环只处理 Cursor。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 工作树 22 项未提交变更均排除；Cursor 执行器、图片、会话及内建工具文件相对 HEAD 无工作树差分。`git log --all` 可见空工具调用、部分超时、导入 token 到期和 Grok 4.7 变体的修复提交，但四者都不是当前 HEAD 的祖先，不把其他分支的代码误记为成熟参考能力。当前 HEAD 的图片入口/压缩约束已有目标对应实现；目标空终态与 marker-only 工具门槛不弱于该 HEAD。多协议部分输出超时仍以显式失败结束，不把非 HEAD 的 Chat 专用 `length` 策略扩成跨协议成功；Grok 4.7 合成模型变体也不凭另一分支代码进入目标公开目录。没有证实新的本地 wire 缺口，`CUR-N2=reviewed_no_wire_delta` 保持，故本轮不改 Cursor 生产代码。OAuth/API Key 两条真实 receipt、厂商接受性和未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。复核收口后下一类别为 Grok。

复核验证：Cursor 关键词 Rust library 338/338、私有 receipt Node 95/95、冻结 Cursor reference-delta `--check-sources`、Server Provider/coverage/UI matrix、文档索引与 `git diff --check` 通过；目标生产代码未变，上轮完整 Rust library 3565/3565 通过的证据仍适用，但本轮不把定向测试冒充新的全量运行。OAuth 和 API Key 两条 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，没有厂商请求。`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.275 Grok 拒答内容应计入可见质量输出

2026-10-13，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 首选且干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `quality_retry_scan.go`：其 Responses message 内容的 `refusal` 字符串计为可见输出，最近 `7f3f3d3c` 还避免从 usage output−reasoning 虚构可见 token。目标本来就只从实际文本计数，不迁移参考的质量 hold/重试、账号池或 token 比例策略；次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 文件排除。目标完整 `refusal` 响应原误记为 `empty`，固定 JSON 反例旧实现先红。现仅在脱敏旁路计数拒答文本，流式 delta、done 与完成快照按优先级避免重复；空占位不妨碍快照补证。既有固定 outcome/长度桶、正文不保留原则不变。不改 HTTP/SSE/WS wire、主终态、Account/Provider/Share 单绑定、同账号恢复、GR-N4 版本/header 或 receipt schema v1/revision 1。离线反例不证明 xAI 真实发送此形状；inference、media、remote_compaction 真实 receipt、GR-N4 同账号证据和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一类别为 Kiro。

验证收口：完整拒答 JSON 的 `empty` 反例在旧实现先红，修复后非流、流式 delta/done、仅 done、空占位后快照与避免重复计数均通过。最终代码完整 Rust 独立串行为 library 3566 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 私有 receipt Node 与全量后端 Node audit/smoke 1909/1909、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、同构建环境预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；未做部署测试。inference、media、remote_compaction 三个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.276 Kiro 字面 tool_use XML 不得泄漏到助手文本

2026-10-13，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`：已提交的非流 `strip_tool_use_xml_leaks` 与流式 `ToolUseXmlLeakFilter` 删除字面 `<tool_use ...>…</tool_use>`，结构化 `toolUseEvent` 单独处理。目标原有过滤器只识别 `<function_calls><invoke>`；同一 EventStream 中字面标签与原生工具事件并存时，前者作为助手文本原样发给客户端，固定非流反例旧实现先红。现先过滤真 `tool_use` XML，再沿用 invoke 救援；跨 chunk 开闭标签、未闭合块和 `<tool_user>` 近似标签已覆盖，过滤器保留容量进入请求生命周期预算。结构化工具 ID/名称/input、去重及无参恢复不变。不改 Kiro/Amazon Q 出站、EventStream CRC/终态/EOF、八种 auth-kind×region Account/Provider/Share 单绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线反例不能证明厂商真实发送；八份真实 receipt、KI-N5 专项证据和当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一类别为 Qoder。

验证收口：实际 EventStream 的字面标签与原生工具并存反例旧实现先红；修复后非流输出只保留可见文字和单个结构化工具，跨 chunk 开闭标签、截断块及近似标签定向 2/2 通过。最终代码完整 Rust 独立串行为 library 3568 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；未做部署测试。八个 auth-kind×region `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.277 Qoder 外层 null 错误字段不得遮住内层限额证据

2026-10-13，按循环只处理 Qoder。`Qoder/AGENTS.md` 首选 TokenRouter 的 `project-doc` 技能在当前会话不可调用；遵守其门禁，未读业务源码、未运行仓库命令或 `--check-sources`，最新业务增量继续 `unreviewed_skill_gate`。只读核对次选 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交错误状态与空流分类：其按状态分类，不能决定目标 Server 已支持的 `112`/`115` 内嵌错误别名优先级；账号池/截断流补终态不迁入。目标 `QoderUpstreamError` 原以第一个存在的顶层 `code`、message、reset 字段为准，显式 `null` 会挡住已支持的 `/error` 或 `/data` 内层值。`code:null` + `error.code=115` 的固定错误体反例先红，限额错误误报普通 400；实际 SSE wrapper 也覆盖同一分类路径。现只让 `null` 回退，非 null 外层字段仍优先，未知外层码不得被内层 `115` 洗成限额；仅既有 `112`/`115` 可出现在下游，message 保持脱敏。不改 COSY 出站、SSE 终态、三 rail × 三 Surface 的 Account/Provider/Share 单绑定、同账号 401 预算或 receipt schema v2/revision 2。离线反例不证明厂商真实发送；三条真实 rail receipt、TokenRouter 合规补审及当前未提交工作树 committed-target observation 仍缺。最终整体 review 未完成，不做 UI 自动化或部署测试；本轮收口后下一类别为 CodeBuddy。

验证收口：固定限额错误反例旧实现先返回 `code=None`、HTTP 400；修复后返回既有 `115`、HTTP 429，message 与 reset 时间也取到原本已支持的内层值。实际 SSE wrapper 分类入口、非 null 顶层 `112` 优先和未知码不降级为 `115` 的正反例通过。最终代码完整 Rust 独立串行为 library 3569 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke（含 Qoder 私有 receipt）、自包含 Qoder reference-delta 与官方 CLI oracle、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。没有运行受 TokenRouter `project-doc` 门禁约束的业务源码 `--check-sources`；完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行，未做部署测试。Global OAuth、Global PAT、CN OAuth 三个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.278 CodeBuddy 拒答分片不得在非流聚合中丢失

2026-10-13，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 WorkBuddy `sse.go`：流式清洗保留非空 `refusal`，非流聚合器却只收集内容与思考；两个未跟踪文档排除。目标流式 decoder 原已保留拒答分片，非流聚合器未收集，两个拒答分片加 `content_filter` 结束的 decoder→aggregator 固定反例在旧实现先红，返回 `message.refusal=null`。现按原顺序收集 `delta.refusal` 到完整回答的 `message.refusal`，纳入既有请求内存预算；上游 `finish_reason` 原样保留。此项只对齐目标流/非流可见语义，不推断新的厂商 wire，不改出站请求、Intl/CN 站点、Account/Provider/Share 单绑定、同账号 401 预算、唯一 `[DONE]`+EOF、receipt schema v2/revision 2 或 CB-N4 disabled。两站真实 receipt、CN 精确模型能力和当前未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一类别为 Antigravity。

验证收口：decoder→aggregator 拒答反例旧实现先红、新实现转绿；完整 Rust library 串行回归 3570 passed / 1 ignored，API 合同 124/124，其他两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。CodeBuddy 私有 receipt Node 186/186、全量后端 Node audit/smoke 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。Intl/CN 两个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入及未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.279 Antigravity 工具配置双拼写不得形成相互矛盾的出站语义

2026-10-13，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 和 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`：前者 Gemini request 转换显式遍历两种函数声明拼写及两种允许名称路径，后者优先提取 `toolConfig` 再回退 `tool_config`。参考均不定义双拼写并存的厂商语义，也不决定目标 Server 的失败策略。目标最终 wire 原只拒绝工具调用/结果 part 的双拼写；工具声明、顶层 `toolConfig`、内层 `functionCallingConfig` 和 `allowedFunctionNames` 的别名冲突只处理优先项，另一套可未校验地出站。四类固定反例中，冲突声明在旧实现先红。现四层都在最终出站前以固定本地错误拒绝；单一驼峰或下划线写法仍通过并保持幂等。此项不推导新 Google wire，不改 endpoint/header、单拼写请求、AG-N6 requestType、AG-D3 replay/compaction、Account/Provider/Share 单绑定、同账号恢复或 receipt schema v2/revision 2。两份真实 OAuth receipt、AG-N6/AG-D3 接受性和未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。本轮验证收口后下一类别为 Claude。

验证收口：四层双拼写反例在旧实现先放行第一项，修复后全拒绝；原始与 v1internal envelope 的单拼写正例及二次规范化幂等通过。Antigravity OAuth 与 AGY OAuth 的 Gemini 最终请求回归均在发网前返回固定、本地且不含凭据的 400。完整 Rust library 串行回归 3572 passed / 1 ignored，API 合同 124/124，其他两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、冻结 Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；未做部署测试。两条 `RUN_REAL=0` 入口分别为 `blocked_inputs/live_pending`，没有真实 Google 请求；离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.280 Claude 原生混合前置 system 不得丢失努力档位与非文本块

2026-10-13，按循环只处理 Claude。只读核对 `Claude/AGENTS.md` 首选且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671`：其 `2d8ec73b` 将 `output_config`、空 content 的 system 视为可跨 user run 保留的 effort directive；参考不定义目标 Server 的前置文本提升，也不能证明所有未来内容块的 Anthropic 接受性。目标原本保留独立空 directive，但 `hoist_leading_text_system_messages` 遇到同一前置 system 消息里的文本、`output_config` 与非文本块时，只要提升了文本就删掉整条原消息。混合消息固定反例在旧实现先红，首条消息直接变为 user。现仅移走非空文本块；同一消息的其他字段及非文本块按原位置留下，必要时保留空 content directive。请求签名仍在改写后计算，纯文本、独立 directive、user 首条和中途 system 的既有路径不变。不改 OAuth endpoint/header/beta、Account/Provider/Share 单绑定、CL-N7/CL-N8、模型目录或 receipt schema v1/revision 1。五份真实 operation receipt、thread/beta 厂商接受性与未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。本轮验证收口后下一类别为 Codex。

验证收口：混合文本、`output_config` 和非文本块的固定反例在旧实现先丢失后两者，修复后保留；确认 Native CLI 的完整 forward-contract 回归验证空 content effort directive 与最终 CCH 一致。完整 Rust library 串行回归 3574 passed / 1 ignored，API 合同 124/124，其他两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、冻结 Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 各为 `blocked_inputs/live_pending`，无真实 Anthropic 请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.281 Codex WS→HTTP 回退中断帧不得意外终止请求

2026-10-13，按循环只处理 Codex。只读核对 `Codex/AGENTS.md` 推荐且干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 和 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233`。前者 `6331b38b` 将 `response.interrupt` 作为独立控制帧，并能在无活动 WS 时交给本地 HTTP 取消；后者近期的原生 WS 续链/保留不定义目标回退路径。目标原来在 WS→HTTP fallback 的请求等待、容量退避和 SSE 读取时，把中断 Text/Binary 帧当成非法并发请求；已输出业务增量后发送未知 ID 中断的回环固定反例在旧实现先红，连接被重置。现仅对 Codex 顶层 `response.interrupt` 返回不含客户端 ID 的本地错误帧，继续原 HTTP 轮次；缺失/非法 ID 也固定拒绝，普通并发 `response.create` 与 Grok 保持原规则。此项不伪装为跨传输取消：匹配 ID 的 HTTP 本地取消、合成 `response.incomplete`、续链上下文及其精确 usage 生命周期仍是待实施差异，需另做端到端验证。出站请求、单账号绑定、同账号恢复预算、Provider outcome、receipt schema v1/revision 1 均未改。五份真实 operation receipt、私有 WS 接受性和未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。本轮验证收口后下一类别为 Cursor。

验证收口：固定未知中断反例旧实现重置连接，新实现返回脱敏错误且同一连接仍应答 Ping；缺失 ID、Binary 中断、非中断帧与非 Codex 路径的局部回归通过。完整 Rust 串行回归为 library 3576 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、冻结 Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.282 Cursor 显式工具选择不得静默放宽

2026-10-13，按循环只处理 Cursor。只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `builtinToolBridge.ts::selectCursorBridgeTools`：无效 OpenAI 指定函数不会扩大为所有候选工具；参考的 Cursor 文件与 HEAD 对象相同，未提交 Codex 工作树内容排除。目标四种入口原可能把显式无效名称或 Gemini `ANY` 多/非字符串允许列表降为 `Auto`/不受限 `Required`，让其他声明工具参与执行。OpenAI Chat 固定反例在旧实现先红；现请求合同及计划构造前置失败关闭，错误固定且不回显客户端内容。四协议合法单名称选择、`none` 和 `required` 保留；Gemini 多名称允许列表在本地无法精确保真，故明确拒绝，不臆测上游会支持。仅改 Cursor 本地工具选择，不改 AgentService protobuf、OAuth/API Key 账号/Share 单绑定、会话续接、同账号恢复、`CUR-N2=reviewed_no_wire_delta` 或 receipt schema v2/revision 2。两份真实 rail receipt、厂商接受性和未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一类别为 Grok。

验证收口：OpenAI Chat 非字符串指定函数反例旧实现先放行、修复后与 Responses、Anthropic、Gemini 的同类反例均在请求准备前拒绝；四协议合法单名称及对象式 `none` 正例通过。完整 Rust library 3578 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、冻结 Cursor reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。OAuth/API Key 两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.283 Grok 空文本占位不得遮住质量快照

2026-10-13，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 首选且干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `quality_retry_scan.go`：只把非空 output-text delta 计为可见输出，输出项另作聚合观察；次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 工作树文件排除。目标 `GrokQualityObserver` 原遇空 `response.output_text.delta` 或空白 `response.output_text.done` 就标记文本来源已出现，完成快照里的真实文本被屏蔽，错误记录 `empty`。空 delta＋空 done＋非空完成快照的固定反例在旧实现先红。现只有非空可见文本才锁定 delta/done 来源；已有有效 delta 继续优先且不重复统计，有效 done 也可在空 delta 后补证。只修脱敏旁路质量观察，不改 HTTP/SSE/WS wire、主成功终态、账号/Share 绑定、同账号恢复、GR-N4 或 receipt schema v1/revision 1。三份真实 operation receipt、厂商接受性和未提交工作树 committed-target observation 仍缺；最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一类别为 Kiro。

验证收口：固定空 delta＋空白 done＋有效完成快照反例旧实现先误判为 `empty`，修复后记为 `text`；空 delta 后有效 done 的正例也通过。完整 Rust library 3579 passed / 1 ignored、API 合同 124/124，另两项集成各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。三个 `RUN_REAL=0` operation 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.284 Kiro 零输入 tokenUsage 快照不得被估算覆盖

2026-10-13，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐且干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`：其 `metadataEvent.tokenUsage` 是单次调用最终快照，缺失输入分项按零处理，重复快照取最后一份；只有没有该快照时才使用 context/本地回退。目标原仅当 tokenUsage 输入合计大于零才写入精确值，因此先前 `contextUsageEvent` 或 `metricsEvent` 的正数可能在最终零输入快照后继续计费，后续 metrics 也能覆盖快照。固定 context＋metrics＋仅含 `outputTokens` 的快照反例在旧实现先红，实际输入报 123 而非 0。现对象型快照的零输入也具权威性，后续回退事件不能覆盖，重复快照可替换上一份且缺失输出/cache 分项归零；上游 cache 零值仍压过本地估算。不改 Kiro/Amazon Q 出站请求、EventStream wire、Account/Provider/Share 与八种 auth-kind×region 固定绑定、KI-N5 context-output shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线反例不证明厂商在八路实际发送该形状；八份真实 receipt、KI-N5 独立计量证据与当前未提交工作树 committed-target observation 仍缺，最终整体 review 未完成，不做 UI 自动化或部署测试。验证收口后下一类别为 Qoder，其首选 TokenRouter 的技能门禁仍需遵守。

验证收口：零输入快照反例旧实现报 123，新实现报 0；后续 metrics/context 不再反盖，第二份非零快照替换前值及第三份缺失输出归零的回归通过。最终代码完整 Rust library 3580 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。八个 `RUN_REAL=0` auth-kind×region 入口均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.285 Qoder 空别名不得遮住有效思考文本

2026-10-13，按循环只处理 Qoder。`Qoder/AGENTS.md` 首选 TokenRouter，但其 `project-doc` 技能当前未加载；遵守门禁，未读其业务文件或运行受限的 `--check-sources`。只读核对次选且业务工作树干净的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `direct.go`：它把 `reasoning_content` 作为规范思考字段，不能据此臆测目标的额外别名是厂商 wire。目标自身兼容合同支持 `reasoning_text`/`reasoning`，但原规范化和非流聚合在 `reasoning_content:null` 时仍选中了空占位，遮住有效低优先级文本。合法业务分片＋单个结束理由＋EOF 的 decoder→aggregator 固定反例旧实现先红，最终响应的思考字段为 `null`。现逐项保留已有限型校验，再取首个非 `null` 别名；直接 Chat 聚合入口也一致，显式空字符串不被下级覆盖。只修下游文本保真，不改 COSY 出站、SSE wrapper/唯一终态+EOF、Account/Provider/Share 与三 rail 固定绑定、同账号恢复或 receipt schema v2/revision 2。离线反例不证明厂商实际发此形状；三条真实 rail receipt、TokenRouter 合规补审、当前未提交工作树 committed-target observation 与最终整体 review 仍缺。不做 UI 自动化或部署测试；验证收口后下一类别为 CodeBuddy。

验证收口：完整 decoder→聚合的空别名反例旧实现将有效思考文本丢为 `null`，修复后保留；直接 Chat 聚合的同形正例也通过。首轮反例曾错误地同时给结束理由和 `[DONE]`，被既有唯一终态门槛正确拒绝，改成单终态＋EOF 后才用于红绿验证。完整 Rust library 3581 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、自包含 Qoder reference-delta、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后的本地服务 smoke 和差分空白检查均通过；生成 coverage 无需改动。未运行 TokenRouter 外部对象 `--check-sources`；完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行，也没有做部署测试。Global OAuth、Global PAT、CN OAuth 三条 `RUN_REAL=0` rail 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.286 CodeBuddy 默认 reasoning 档位不得被空值遮住

2026-10-13，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐且业务工作树干净的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `catalog.go::catalogModel`：规范化后的 `reasoning.defaultEffort` 为空时回退到 `reasoning.effort`；两个未跟踪文档排除。目标原在原始值层面先选 `defaultEffort`，若它是 `null` 或空白，后续有效 `effort` 被遮住；客户端明确请求 `reasoning:{}` 的最终 WorkBuddy payload 因此缺少 `reasoning_effort`。目录解析→请求构造的两种固定反例旧实现先红。现仅在首字段无有效文本时回退；显式非字符串首字段仍不回退，非空首字段仍优先，显式请求 effort 与目录菜单校验、reviewed 模型白名单及“未请求不自动开启 reasoning”不变。不改上游 endpoint/header、Intl/CN 站点、Account/Provider/Share 单绑定、同账号恢复、唯一 `[DONE]`+EOF、CB-N4 disabled 或 receipt schema v2/revision 2。离线反例不证明厂商真实接受；两站真实 receipt、CN 精确模型能力、当前未提交工作树 committed-target observation 和最终整体 review 仍缺。不做 UI 自动化或部署测试；验证收口后下一类别回到 Antigravity。

验证收口：目录 `defaultEffort:null` 与空白字符串两种固定反例，旧实现不能从 `effort:"high"` 生成显式请求所需档位，修复后均输出顶层 `reasoning_effort:"high"`；非空首字段优先、显式非字符串不回退和未请求不自动启用的边界回归通过。完整 Rust library 3582 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。全量后端 Node audit/smoke 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、预构建后本地服务 smoke 与差分空白检查均通过；生成 coverage 无需改动。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。Intl/CN 两个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.287 Antigravity replay 签名别名不得被空值遮住

2026-10-13，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐且干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`。前者 replay 原生签名读取逐项跳过空文本，后者识别驼峰及下划线拼写但不能证明空值回退；均不能据此推断 Google 真实发送多别名并存的形状。目标 `native_signature` 原先选第一个存在的字段再校验，`thoughtSignature:null` 会遮住有效 `thought_signature` 或 `extra_content.google.thought_signature`。非流/流式响应漏存签名，以及续轮请求在已有异值原生签名旁写入缓存签名、形成冲突的固定反例均在旧实现先红。现仅逐项应用原有有效签名门槛，取首个有效值；既有有效优先字段优先，异值冲突仍失败关闭。不改出站 endpoint/header、单拼写合法请求、AG-N6、AG-D3/compaction、Account/Provider/Share 单绑定、同账号恢复或 receipt schema v2/revision 2。两条真实 OAuth rail receipt、AG-N6/AG-D3 专项证据、当前未提交工作树 committed-target observation 与最终整体 review 仍缺。不做 UI 自动化或部署测试；验证收口后下一类别为 Claude。

验证收口：非流、流式捕获和续轮冲突两条生产路径反例在旧实现先红，修复后 Antigravity replay 定向 26/26 通过。完整 Rust 串行回归为 library 3584 passed / 1 ignored、API 合同 124/124、另两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）；后端 Node audit/smoke 1909/1909、冻结 Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、本地 API 服务 smoke 和差分空白检查均通过；生成 coverage 无需手改。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。`antigravity_oauth` 和 `agy_oauth` 两个 `RUN_REAL=0` 入口都为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.288 Claude 嵌套工具引用须与声明别名一致

2026-10-13，按循环只处理 Claude。只读核对 `Claude/AGENTS.md` 首选且干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671`：其已提交 Claude 请求/响应映射覆盖 `tool_result.content[]` 与 `tool_search_tool_result.content.tool_references[]` 内的 `tool_reference.tool_name`，但不能据此推断厂商对未来嵌套形状的接受性。目标 Claude OAuth 已将非法 custom/MCP 工具名变为请求内别名，却只重写顶层历史引用；实际最终请求准备中，声明变成 `cc_tool_*`、嵌套引用仍为原名的固定反例旧实现先红。回程 JSON 的顶层与嵌套引用也未恢复原名，固定反例先红。现只匹配上述协议位置重写出站引用，并在响应恢复中处理 `tool_reference.tool_name`；同一 SSE patcher 复用此规则。未声明引用、普通文本同名字段和工具 `input` 原样保留，映射仍在最终 CCH 前。不启用原生 tool-search，不改 OAuth endpoint/header/beta、Account/Provider/Share 单绑定、CL-N7/CL-N8 或 receipt schema v1/revision 1。五项真实 operation receipt、thread/beta/工具引用厂商接受性、未提交工作树 committed-target observation 和最终整体 review 仍缺。不做 UI 自动化或部署测试；验证收口后下一类别为 Codex。

验证收口：实际 `apply_forward_contract` 的嵌套请求反例、JSON 回程引用反例均在旧实现先红，修复后两项通过；另加 SSE 多处分块的嵌套引用恢复测试。完整 Rust 独立串行回归为 library 3587 passed / 1 ignored、API 合同 124/124、另两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。后端 Node audit/smoke 1909/1909、冻结 Claude reference-delta `--check-sources`、离线 Claude wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、本地 API 服务 smoke 和差分空白检查通过；生成 coverage 无需手改。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有做部署测试。五个 `RUN_REAL=0` operation 都为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.289 Codex WS→HTTP 回退匹配中断的本地取消

2026-10-13，按循环只处理 Codex。只读复核 `Codex/AGENTS.md` 推荐且干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233`。前者已提交 `6331b38b` 对无活动上游 WS 的中断使用本地 HTTP 取消器，发 `reason=interrupted` 的 `response.incomplete`；其实现不核对响应 ID，不能照搬成目标的匹配规则。后者近期 WS 连接续链与上下文保留不定义 HTTP 回退取消。目标 §18.281 已保证未知中断不重置连接，但匹配 ID 同样被拒绝。已输出业务增量后发送匹配 ID 的实际回环反例旧实现先红。现仅在已观测到本轮 `response.created.response.id` 且 ID 精确匹配时停止本轮 HTTP 流，补发尚未提交的生命周期帧，按已有已完成输出项构造 `response.incomplete`，清理输出缓存并让同一客户端 WS 继续下一轮；未知、非法或未观测 ID 保持脱敏拒绝。用量记 499/`client_cancelled`，不把主动取消报成 Provider 成功/失败。不伪造未观测最终 usage 或未完成输出项，不保证中断 ID 可用于私有上游续链。出站请求、Account/Provider/Share 单绑定、同账号恢复与 receipt schema v1/revision 1 不变。五份真实 operation receipt、私有 WS 接受性、当前未提交工作树 committed-target observation 与最终整体 review 仍缺；不做 UI 自动化或部署测试。本轮验证收口后下一类别为 Cursor。

验证收口：匹配 ID 回环在旧实现只收到本地 `error`，修复后两轮都收到 `response.incomplete`，同一连接发起第二轮并产生两条最终 499/`client_cancelled` 用量记录；未知 ID 保持活动轮次及原 HTTP 读取。Codex HTTP fallback 定向 22/22 通过；完整 Rust library 3588 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1。后端 Node audit/smoke、Codex reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、本地服务 smoke 与差分空白检查通过；生成 coverage 无需手改。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行，也没有做部署测试。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。本轮未提交，不追加 committed-target observation。

### 18.290 Cursor Connect 内嵌额度错误不能误作网络故障

2026-10-13，按循环只处理 Cursor。只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 提交态 `cursor.ts::processFrame/tryParseJsonError` 与 `cursorErrors.ts::classifyCursorErrorKind`；相关 Cursor 文件在参考工作树中无差异，未提交 Codex 变更排除。参考把 Connect 数据帧的 JSON 错误与 protobuf 区分，并对 `resource_exhausted` 区分额度和请求大小；其账号池、客户端错误文本和通用恢复策略不迁入目标。目标原把 gRPC 状态 8 和 Connect 终态的同码拒绝报为 502/`NetworkFailure`，JSON 错误帧可能变成 protobuf 解码或空回答错误。gRPC 和 Provider outcome 两条固定反例旧实现先红。现识别至多 8 KiB 的顶层 JSON `error`、Connect 终态错误码及 gRPC 状态 8：额度返回 429，Provider outcome 为 `RateLimited`，只给当前绑定账号冷却；明确请求过大且无额度线索为 400，未知 JSON 错误固定 502。重试时间继续有界解析，不回显上游自由文本；流式已有部分输出时发送错误而非成功终态。非零其他 gRPC 状态、出站 AgentService wire、OAuth/API Key 凭据、Account/Provider/Share 单绑定、会话续接、`CUR-N2=reviewed_no_wire_delta` 和 receipt schema v2/revision 2 不变。离线反例不证明厂商真实发送这些错误；两份 rail receipt、当前未提交工作树 committed-target observation 与最终整体 review 仍缺，不做 UI 自动化或部署测试。本轮收口后下一类别为 Grok。

验证收口：Cursor 模块定向 279/279、Codex HTTP fallback 定向 22/22、完整 Rust library 3591 passed / 1 ignored；后端 Node audit/smoke、Cursor reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、文档索引、严格 Clippy、`cargo check -q`、本地服务 smoke 在本轮此前通过，未跑包含 Web UI 自动化的完整 `scripts/static-checks.sh`。完整 Rust 首次发现 Codex 匹配中断两轮复用测试偶发断开：测试夹具第二轮刷新目标时丢掉本地 WS URL，现把本地地址保存在测试 runtime plan，刷新仍保持本地；不是生产路由变更。另默认测试线程栈下 `api::providers::tests::codebuddy_provider_test_is_local_on_dry_run_and_reaches_strict_driver_for_all_surfaces` 单独运行会栈溢出，`RUST_MIN_STACK=16777216` 下单独及完整 library 均通过；故完整通过只对应明确增栈的测试配置，默认栈问题留待 CodeBuddy 类别处理。两条 Cursor `RUN_REAL=0` 仍为 `blocked_inputs/live_pending`，离线 readiness 因缺真实输入且禁做部署测试按预期阻断，不是发布通过。本轮未提交、未推送。

### 18.291 Grok 私有 doom-loop 控制帧不得泄漏给 Responses 客户端

2026-10-13，按循环只处理 Grok。只读核对 `Grok/AGENTS.md` 首选且干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `responses_response.go::isPrivateBuildControlEvent` 与 `responses_advanced_test.go::TestResponsesStreamAlwaysStripsBOMAndDoomLoopControlEvents`；参考将 Build 的 `response.doom_loop_check` 私有 SSE 控制事件从兼容输出中过滤。次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 六项未提交 Codex 文件排除。目标 Grok SSE 检查器原只规范化 ping，私有事件原样下传；分片固定反例旧实现先红。现仅在 JSON 对象 `type` 精确匹配、可选 SSE `event` 同名且无错误/失败/终态语义及 `id`、`retry`、未知字段时改为无数据注释；歧义帧保留给主语义门禁。实际 HTTP 回环证明合法文本和完成终态仍可见，而私有帧不出现在 Codex Responses 客户端。只修 Grok OAuth Responses SSE 入站，不改 HTTP JSON/WS、出站 wire、Account/Provider/Share 单绑定、同账号恢复、搜索/质量成功门槛、GR-N4 或 receipt schema v1/revision 1。参考的 Build 私有事件不证明目标 direct-XAI OAuth rail 实际发送；三份真实 operation receipt、GR-N4 同账号厂商接受性、当前未提交工作树 committed-target observation 及最终整体 review 仍缺。禁止 UI 自动化和部署测试；本轮收口后下一单类别为 Kiro。

验证收口：私有控制帧的每字节分片、CRLF/裸数据、错误/重连元数据保真单元反例先红后绿；Grok OAuth 实际 HTTP SSE 回环确认客户端只收到业务文本和完成终态。最终代码 `RUST_MIN_STACK=16777216 cargo test -q` 全通过：library 3593 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1；此配置保留上一轮发现的 CodeBuddy 默认测试线程栈限制。后端 Node audit/smoke 1909/1909、Grok 私有 receipt 夹具 159/159、冻结 Grok reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings` 与差分空白检查通过；生成 coverage 无需手改。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发送真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入且禁做部署测试按预期返回 `blocked_inputs`，不是发布通过。完整 `scripts/static-checks.sh` 含 Web UI 自动化，按禁令未运行；本轮未做本地服务 smoke、UI 自动化或部署测试，未提交、未推送。

### 18.292 Kiro 权威 tokenUsage 不得把畸形数字截断或伪装成零

2026-10-14，按循环只处理 Kiro。只读核对 `Kiro/AGENTS.md` 推荐且干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/kiro/model/events/metadata.rs::TokenUsage`：上游权威计数是带缺省零的 `i32`，不是可截断的浮点。目标 `metadataEvent`/`messageMetadataEvent` 原把浮点计数截断成整数；对象内字符串、越界值可能被当作零值权威快照，非对象 `tokenUsage` 则可能被当作无快照回退。实际 EventStream 分数输出计数反例在旧实现先红。现于共用 wire 解析入口要求显式计数为 `i32` 范围内的 JSON 整数，对既有 camel/snake 别名一起校验；整体 `tokenUsage:null` 和缺失字段原规则保持，合法负整数仍由最终用量归零。畸形快照固定、脱敏地以 `KIRO_EVENT_STREAM_INVALID` 终止；非流与流式都不把它变成成功终态。只修入站权威计量形状，不改出站 Kiro/Amazon Q wire、EventStream CRC/容量、Account/Provider/Share 与八路 auth-kind×region 固定绑定、同账号恢复、KI-N5 context-output shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。八份真实 receipt、KI-N5 专项厂商证据、当前未提交工作树 committed-target observation 和最终整体 review 仍缺。禁止 UI 自动化与部署测试；本轮收口后下一类别为 Qoder。

验证收口：实际 EventStream 的小数计数反例旧实现先接受，修复后分数、浮点整数写法、字符串、越界、显式 `null` 字段和非对象快照均拒绝；整数别名、合法负数、整体 `tokenUsage:null` 与无快照保持可用。流式/非流式闭环均以 `KIRO_EVENT_STREAM_INVALID` 结束，流式不发送 `message_stop`。`RUST_MIN_STACK=16777216 cargo test -q` 全通过：library 3595 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1；明确增栈是此前 CodeBuddy 默认测试线程栈问题的现有验证配置。Kiro 私有 receipt 离线夹具 492/492、冻结 Kiro reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings` 与差分空白检查通过；生成 coverage 无需手改。八个 `RUN_REAL=0` scope 均为 `blocked_inputs/live_pending`，未发真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因跳过内置测试、缺真实输入且禁做部署测试按预期阻断，不是发布通过。完整 `scripts/static-checks.sh` 含 Web UI 自动化，按禁令未运行；本轮未做本地服务 smoke、UI 自动化或部署测试，未提交、未推送。

### 18.293 Qoder HTTP 200 内层错误不得被成功终态遮蔽

2026-10-14，按循环只处理 Qoder。`Qoder/AGENTS.md` 首选 TokenRouter，但其 `AGENTS.md` 要求当前会话不可用的 `project-doc` 技能；遵守门禁，未读其业务源码或运行访问外仓源码的 `--check-sources`，最新业务增量继续为 `unreviewed_skill_gate`。只读核对次选、已提交的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 中 `direct.go::upstreamError/collectUpstream/writeOpenAIChunks`：参考会观察 HTTP 200 SSE body 里的 `error`；未跟踪的 `proxy.html`、`proxy.md` 排除，账号池和宽松终态不迁入。目标原把内层 `error` 当成无意义事件丢弃，后续成功终态可能把额度错误洗成回答；`115` 固定反例旧实现先红。现非 `null` 的内层顶层 `error` 在业务规范化前由既有上游错误分类处理，已知 `112`/`115` 分别为权限/限额，无法分类且无限额时间线索的错误为 502；即使同帧有成功 choice、此前已有部分输出也不能覆盖失败。`error:null` 的正常业务保持。仅修 Qoder 入站，不改 COSY 出站、Global OAuth/Global PAT/CN OAuth 三 rail 固定绑定、同账号恢复、唯一终态+EOF 或 receipt schema v2/revision 2。离线反例不证明厂商真实错误形状；三条真实 receipt、TokenRouter 合规补审、当前未提交工作树 committed-target observation 及最终整体 review 仍缺。不做 UI 自动化或部署测试；本轮收口后下一类别为 CodeBuddy。

验证收口：HTTP 200 内层 `115` 反例旧实现先接受、修复后按 429 失败；结构化、字符串、空对象错误、同帧成功 choice、前序部分输出与 `error:null` 正例均通过。`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 全通过：library 3596 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1；明确增栈沿用既有 CodeBuddy 默认测试线程栈限制。Qoder 私有 receipt 离线夹具 281/281、全量后端 Node audit/smoke 1909/1909、自包含 Qoder reference-delta、冻结 CLI oracle、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings` 与差分空白检查通过；生成 coverage 无需手改。受 TokenRouter `project-doc` 门禁约束的 `--check-sources` 未运行。Global OAuth、Global PAT、CN OAuth 三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，未发送真实请求。完整 `scripts/static-checks.sh` 含 Web UI 自动化，按禁令未运行；本轮未做本地服务 smoke、UI 自动化或部署测试，未提交、未推送。

### 18.294 CodeBuddy 非流收尾不得耗尽默认测试线程栈

2026-10-14，按循环只处理 CodeBuddy。只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/workbuddy/sse.go::Aggregate/rewriteChatStream`；工作树两个未跟踪文档排除。参考区分非流聚合与流式清洗，但不定义本 Server 的共享转发、usage durable commit 或线程栈，不据此引入新 WorkBuddy wire。此前待办的默认 2 MiB 栈 CodeBuddy Provider 三 Surface 自测单独复现 SIGABRT，3 MiB 通过；GDB 把溢出定位在 `forward_with_attempt` → `forward_codebuddy` → `finish_qoder_nonstream` → `log_usage` → `commit_usage_append` 的嵌套 poll。仅堆化测试 future 或仅堆化收尾 future 仍失败。现只在 CodeBuddy 非流已完成上游聚合后，用独立 Tokio 任务执行共享收尾并等待结果；复用既有 abort guard，在调用方取消时中止任务，Account/Share guard 随任务释放。默认栈反例先红后绿，默认栈 library 全套通过；不改变出站、SSE、错误分类、同账号恢复预算、Intl/CN 单账号绑定、receipt schema v2/revision 2 或 CB-N4 disabled。调试态默认栈问题不证明生产环境实际栈溢出。两站真实 receipt、CN 精确模型能力、当前未提交工作树 committed-target observation 与最终整体 review 仍缺。完整默认栈 `cargo test` 另在 Claude/Kiro API 合同用例溢出，留待对应类别；本轮收口后下一类别为 Antigravity。

验证收口：CodeBuddy Provider 三 Surface 单测默认栈旧实现 SIGABRT，最终实现通过；默认栈 library 3596 passed / 1 ignored。`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 全通过：library 3596 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1。CodeBuddy 私有 receipt 离线夹具 186/186、全量后端 Node audit/smoke 1909/1909、冻结 CodeBuddy reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings` 与差分空白检查均通过；生成 coverage 无需手改。Intl/CN 两个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发送真实请求。完整 `scripts/static-checks.sh` 含 Web UI 自动化，按禁令未运行；本轮未做本地服务 smoke、UI 自动化或部署测试，未提交、未推送。

### 18.295 Antigravity Gemini 尾部 assistant 纯思考回合不得残留

2026-10-14，按循环只处理 Antigravity。只读核对 `Antigravity/AGENTS.md` 推荐且干净的两个参考：`CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 的已提交 `a2718314` 在最终 Gemini 内容边界清理 `model` 或 `assistant` 纯思考尾回合；`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的 thinking store 区分 `thought:true` 与工具/正文，但没有对应尾部删除规则。目标已清理 `model`，却漏掉自身其他尾部判断也接受的 `assistant`。`user` 后跟签名 assistant 纯思考回合的最终 v1internal wire 反例旧实现先红，尾部原样保留。现仅把纯思考尾部谓词扩展到 `assistant`；仍要求每个 part 显式 `thought:true` 且只有思考文本/签名字段、至少保留一个回合。带正文、工具或媒体的 assistant 回合及 Claude 目标不删；正常工具名称/ID 规范化继续运行。此项只修 Gemini 目标最终出站内容，不改 endpoint/header、Account/Provider/Share 单绑定、AG-N6 requestType、AG-D3 replay/compaction 或 receipt schema v2/revision 2。离线构造不证明双 rail 厂商实际发送或接受；Antigravity/AGY 真实 receipt、专项接受性、当前未提交工作树 committed-target observation 和最终整体 review 仍缺。不做 UI 自动化或部署测试；本轮收口后下一类别为 Claude。

验证收口：签名 assistant 纯思考尾回合反例旧实现保留、修复后移除；连续两个尾回合同样移除，混合正文/工具/媒体及 Claude 目标正例保持。测试曾错误要求工具回合整对象不变，与既有工具 ID 规范化冲突；现改为核对回合及 part 未被删除。Antigravity 定向 Rust 117/117；默认栈完整 library 3597 passed / 1 ignored，增栈 `CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 完整通过：library 3597 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1。Antigravity 私有 receipt 离线夹具 165/165、全量后端 Node audit/smoke 1909/1909、冻结 Antigravity reference-delta `--check-sources`、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings` 与差分空白检查通过；生成 coverage 无需手改。双 rail `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，未发送真实请求。完整 `scripts/static-checks.sh` 含 Web UI 自动化，按禁令未运行；本轮未做本地服务 smoke、UI 自动化或部署测试，未提交、未推送。

### 18.296 Claude OAuth Responses 缺省输出上限遵循模型能力

2026-10-14，按循环只处理 Claude。只读核对 `Claude/AGENTS.md` 首选且干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 `a881c036`：Responses 缺省 `max_output_tokens` 按已注册非 Fable 模型的 completion 上限设定，Fable 默认保守 64,000；参考 Sonnet 4.6 目录值与目标不同，不能照抄数值。目标共享 OpenAI→Anthropic 转换默认填 8,192，Claude OAuth 后续缺省填充因字段已存在而失效。实际 Codex Responses→Claude OAuth adapter 反例旧实现先红，Opus 5.5 缺省得到 8,192。现只在该入口、所有兼容上限字段缺失或为 `null` 时，转换前按目标静态模型目录填充值：非 Fable 用目录上限，Fable 限 64,000，未知模型仍沿用 8,192。显式值、Chat、Claude API Key、其他 Provider 和原生 Claude 请求不变；推理预算可读取正确上限。不改 endpoint/header、Account/Provider/Share 单绑定、原生 tool-search disabled 决策、CL-N7/CL-N8、模型目录或 receipt schema v1/revision 1。静态目录不证明真实订阅接受性；五份真实 operation receipt、当前未提交工作树 committed-target observation 和最终整体 review 仍缺。不做 UI 自动化或部署测试；本轮收口后下一类别为 Codex。

验证收口：固定反例旧实现先红，修复后覆盖 Opus 缺省/`null`、Fable 64,000、未知模型 8,192、显式 4,096 与非 OAuth Provider 保持 8,192。`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 完整通过：library 3598 passed / 1 ignored、API 合同 124/124、另两个集成目标各 1/1。Claude 私有 receipt 离线夹具 252/252、全量后端 Node audit/smoke 1909/1909、Claude reference-delta `--check-sources`、离线 wire profile、Server Provider/Phase 0/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings` 与差分空白检查均通过；生成 coverage 核对无需手改。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求；`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 因主动跳过内置测试、缺真实输入和未做部署验收按预期阻断，不是发布通过。完整 `scripts/static-checks.sh` 含 Web UI 自动化，按禁令未运行；本轮未做本地服务 smoke、UI 自动化或部署测试，未提交、未推送。

### 18.297 Codex 客户端内建整数工具字段的精确嵌套路径

2026-10-14，按循环只处理 Codex。只读核对 `Codex/AGENTS.md` 推荐且干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233`：前者已提交 `d7914afd` 扩充 Codex 客户端→非 Codex executor 的已知整数字段映射，新增 notes/history/memories/web/collaboration 的精确嵌套路径；后者近期 WS/身份增量不定义该修正。目标此前完成 CX-D3 第一批，但只按少数工具和顶层属性匹配，且递归 namespace 时不核对 namespace 名称。`notes__read_file.start_line.anyOf[0]` 仍为 `number`、任意 namespace 的 `wait_agent.timeout_ms` 可能误变 `integer`；固定正反例旧实现先红。现扩充参考列出的静态工具/字段路径，按 namespace 组合名称，只沿白名单中的对象/数组位置改 `type`，保留 schema 深度和容量门禁。官方 Codex UA、非 Codex Provider、Codex 路由三重范围不变；普通数值、未知工具/namespace、参数内容、Codex OAuth 原生目标和非官方客户端不动。不改 OAuth endpoint/header、Account/Provider/Share 单绑定、同账号恢复、CX-N13、Images/WS prewarm disabled 决策或 receipt schema v1/revision 1。离线 schema 不证明各目标上游接受性；五份真实 operation receipt、当前未提交工作树 committed-target observation 和最终整体 review 仍缺。不做 UI 自动化或部署测试；本轮收口后下一类别为 Cursor。

验证收口：嵌套 `anyOf`、web 数组路径与 collaboration namespace 正例，以及未知 namespace、普通数值字段、非官方客户端和 Codex executor 反例均通过；本次续跑 Codex 定向 Rust 3/3、`cargo check -q` 与严格 Clippy `--all-targets -D warnings` 通过。此前同一工作树的完整 Rust 为 library 3599 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1；Codex 私有离线夹具 205/205、全量后端 Node audit/smoke 1909/1909，格式、冻结参考差分、coverage、合同、文档索引和差分空白检查均通过。真实账号 receipt 属于后续独立验收，不作为本 goal 本地实施完成的阻断条件；未提交工作树仍需在最终证据复核中如实标注，整体 review 尚未完成。没有运行 UI 自动化或部署测试，本轮未提交、未推送。

### 18.298 Cursor 当前提交态与本地计划收口复核

2026-10-14，按循环只处理 Cursor。`Cursor/AGENTS.md` 首选的 `OmniRoute` 当前 `HEAD=443d66996d69d7ecf887fd61158404a3cca1b192`，与既有冻结点相同；22 项未提交改动仍未触及 Cursor 执行器、协议、工具桥接、图片或会话文件，全部排除。冻结 Cursor reference-delta 的十个 legacy 字段、八条不可变 observation 与 `CUR-N2` 无 wire 差分复核继续成立。逐项回看参考 `cursor.ts` 的 Connect JSON 错误、空终态、KV/Exec 事件、工具终态和会话清理，与目标的 `agent_driver`、`agent_proto`、`session`、`request_builder`、`image` 现行实现及 §18.250、§18.258、§18.282、§18.290 后续修正：没有发现新的已提交参考行为缺口。参考在部分输出后把内嵌错误当作干净终止的宽松做法不迁入；目标保持显式失败和固定 Account/Provider/Share 绑定。故本类别无新增生产代码，计划内可离线实施项维持已完成；OAuth/API-key 两条真实 receipt 仍如实标记 `live_pending`，但采集不属于本 goal。未提交工作树的 committed-target observation 只能等提交态证据复核，最终整体 review 尚未完成。

本轮验证：Cursor Rust library 定向 343/343、API 合同 124/124、Cursor 私有 receipt 离线测试 95/95、冻结参考差分 `--check-sources`、Server Provider coverage/UI matrix 和 `git diff --check` 均通过。以上不证明厂商真实接受性，也不代表新的全量 Rust 运行；没有执行 UI 自动化或部署测试，本轮未提交、未推送。下一类别为 Grok。

### 18.299 Grok Responses SSE 流首 BOM 不得吞掉首个事件

2026-10-14，按循环只处理 Grok。首选且干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `responses_response.go::normalizeResponseStream` 和 `responses_advanced_test.go::TestResponsesStreamAlwaysStripsBOMAndDoomLoopControlEvents` 会在兼容流边界移除 UTF-8 BOM；次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 修改排除。参考不证明目标 direct-XAI OAuth rail 实际发送 BOM，但流首 BOM 是确定的 SSE 解析边界。目标 Grok SSE 包装层原样下传 BOM，紧贴首个 `data:` 时共享解码器将该字段当作未知字段，唯一 `response.completed` 可丢失；固定分片反例在旧实现先红。现仅在 Grok OAuth Responses SSE 流首探测最多三个字节，完整 BOM 才移除；未匹配前缀、流中间 BOM 和 EOF 不完整前缀保持原样，交给主协议门禁。探测缓冲纳入同一请求内存预算，搜索/质量观察和主终态仍按原规则。不改出站 wire、HTTP JSON/原生 WS、Account/Provider/Share 单绑定、同账号恢复、GR-N4 版本/header、remote compaction disabled 或 receipt schema v1/revision 1。`PROTOCOL_EVIDENCE.md` 与 Grok Provider 文档已同步；真实账号 receipt 是本 goal 外的后续验收，状态继续 `live_pending`，最终整体 review 未完成。

验证收口：完整 BOM 在任意两块边界均被移除，主 Responses 检查器确认唯一完成终态；非流首匹配前缀、流中间 BOM 与 EOF 半截前缀不被静默删除。`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 全通过：library 3600 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1。Grok 私有 receipt 离线测试 159/159、冻结 Grok reference-delta `--check-sources`、Provider coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings` 与差分空白检查均通过。完整 `scripts/static-checks.sh` 含 UI 自动化，按禁令未运行；没有做部署测试，本轮未提交、未推送。下一类别为 Kiro。

### 18.300 Kiro/Amazon Q Compact 禁用门禁覆盖两种入口与流式矩阵

2026-10-14，按循环只处理 Kiro。首选且干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的 `responses_compaction.rs` 把末尾唯一 `compaction_trigger` 作为独立 RemoteCompact 操作，内部强制非流摘要并另行映射 compact 输出；这再次证明普通生成 executor 不能代替 Kiro Compact。目标已在 `forward_with_attempt` 发网前调用 KI-N3 禁用门禁，但既有零发网回环只覆盖显式 `/compact`。现扩充同一真实转发入口的 decoy 矩阵：Kiro 与 Amazon Q 分别验证 `/compact` 和普通 Responses 末尾 `compaction_trigger`，每种再覆盖 `stream=false/true`，八个请求均稳定 400，catalog、token 和 inference decoy 合计零请求。生产代码、出站 wire、Account/Provider/Share 单绑定、同账号恢复、KI-N5 usage shadow、shared cache 与 receipt schema v1/revision 1 均不变。此项只加强 KI-N3 P0 的离线 fail-closed 证明；P1 的专属 Compact executor 仍不启用，厂商真实 receipt 另行验收，不能将禁用测试冒充 Compact 能力通过。最终整体 review 尚未完成。

验证收口：扩充后的 KI-N3 回环 1/1，Kiro Rust 定向 228/228、八路私有 receipt 离线测试 492/492；`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 全通过：library 3600 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1。冻结 Kiro reference-delta `--check-sources`、Provider coverage、`cargo fmt --check`、严格 Clippy `--all-targets -D warnings` 与差分空白检查通过。完整 `scripts/static-checks.sh` 含 UI 自动化，按禁令未运行；没有做部署测试，本轮未提交、未推送。下一类别为 Qoder。

### 18.301 Qoder 首选参考门禁重核

2026-10-14，`Qoder/AGENTS.md` 仍只推荐优先参考 TokenRouter。其链接目标 `/data/projects/proxy/TokenRouter` 当前工作树已删除根目录 `AGENTS.md`，从该仓根目录到 `backend/internal/upstream/qoder/` 也没有其他适用的 `AGENTS.md`；因此 §17.9 所记的 `project-doc` 阻断不再适用于**当前工作树**。删除仍是未暂存的 ` D AGENTS.md`，`TokenRouter@c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的已提交树仍含旧文件；不可把删除说成提交态事实，文件恢复时须重核指令。既有 `QODER-SKILL-GATE-2026-09-28` 是历史观察，不改写。当前已运行只读 `node scripts/audit/audit-qoder-reference-delta.mjs --check-sources` 并通过；它只复核冻结的 `TokenRouter@7faf9469bc695` 对象，不证明 `6d676f1d100e11f2f068949f5779e960c862079c..c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的 Qoder 业务增量无差异。该区间含大量 Qoder 实现、模型目录与 gateway 改动，需继续按已提交对象逐项补审，再决定是否修目标实现或追加新证据。

真实账号 receipt 采集与厂商验收不在本 goal 内；Global OAuth、Global PAT、CN OAuth 的 `live_pending` 继续如实保留，也不因离线补审通过而启用未验证能力。当前 Qoder 增量补审及最终整体 review 均未完成；本节只是移除已失效的本地技能门禁，不是 Qoder 类别收口。

### 18.302 Qoder committed-source 补审首批差异与 Responses 续接缺口

2026-10-14，只处理 Qoder。参考 `TokenRouter@7faf9469bc695` 的已提交 `backend/internal/service/qoder_gateway_service.go` 已有 `previous_response_id` 解析、响应 ID→会话别名、追加式会话计划；当前 `TokenRouter@c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的已提交 `backend/internal/upstream/qoder/{payload.go,conversation.go,execute.go}` 将它迁至新模块，并在 `Execute` 的 Responses 路径继续启用。目标 `src/proxy/transforms.rs::openai_responses_to_chat_with_reasoning_effort` 生成 Chat 时不保留 `previous_response_id`，`src/proxy/forwarder.rs::forward_qoder` 与 `src/proxy/providers/qoder/mod.rs::prepare_canonical_request` 只见转换后的 body，也没有 Qoder 续接别名；因此当前 Codex Responses 客户端声明该字段时可被静默当成新会话。这是协议语义缺口，不可用现有 Qoder 111 项定向 Rust 和 281 项私有 receipt 离线测试通过来宣称覆盖。

**QD-N3 待实施**：先在 Qoder 原始 Responses 入口固定流/非流反例，证明 `previous_response_id` 不得静默丢失；随后按本 Server 的单账号边界实现精确响应 ID→COSY 会话映射，作用域包含 Share、用户、Provider revision/runtime、Account 身份/代际与实例、站点、credential rail、模型，状态须有 TTL、条数和内存上限。仅在唯一终态与 EOF 且输出成功后发布下一 ID；缺失、过期、歧义、身份漂移或续接所需历史不可用时发网前明确失败，不跨账号/站点/rail fallback，不把客户端 ID 裁剪成另一个身份。回环至少覆盖首轮→第二轮只追加新 input、工具结果、系统/工具继承、错误/取消不发布、并发冲突与两代际隔离，并逐个核对 Claude/Gemini 和无续接 Responses 正例。若实现前需要暂时阻断该字段，应把显式拒绝仅作为中间安全门槛，不能冒充 QD-N3 完成。当前不启用任何未经证据支持的新 COSY capability，三 rail `live_pending` 不变。

同区间其他已定位差异：`5ec14856e` 移除静态目录隐式白名单、`3bb44a6d3` 把 Qoder 1.24.2 模型属性和管理员覆盖接入请求，目标已有按绑定账号实时目录的精确启用门禁、同站别名和保守能力矩阵，不能用较新版本的静态值覆盖冻结 1.1.32 oracle 或放宽权威空目录；`4440740a5` 区分上游自报模型与合成的下游别名，目标 Qoder 尚无可信自报模型输入，不能把合成模型写作厂商观察；`871edf920` 将流式上游执行与客户端断连分离、保留内部终止以收集尾部 usage，目标目前由下游 body 拥有上游流，断连会走中断结算。后者需进一步核对后台收集、请求超时、资源预算和单绑定守卫的完整生命周期，不能直接照搬脱离取消的 Go context。以上仍是补审中的差异分类，不是当前 HEAD 全部 Qoder/gateway 改动的审计收口。

本轮只读验证：冻结 Qoder reference-delta `--check-sources` 通过；`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q qoder` 为 library 111/111、API 合同筛选 1/1；`node --test scripts/audit/qoder-real.test.mjs` 为 281/281；计划文件 `git diff --check` 通过。没有运行真实账号、UI 自动化或部署测试，没有提交、推送。QD-N3 与断连结算差异尚未实施，不能把上述现有检查当成新能力证明。

### 18.303 Qoder Responses 未实现续接的生成前失败关闭

2026-10-14，只处理 Qoder。先在真实 Qoder HTTP 转发入口加入 Global OAuth、CN OAuth、Global PAT × 流/非流六组未知 `previous_response_id` 反例；旧实现第一组返回 200，证明 Responses→Chat 转换静默丢弃续接声明并照常生成。现仅在 Qoder Codex Responses 的原始 JSON 已由 adapter 解析、但尚未转换为 Chat 前检查该字段：缺失和 `null` 不变；非空、至多 512 字节、无首尾空白的字符串因为当前没有作用域化别名而固定返回 409 `response_context_unavailable`；非字符串、空值、边界空白和超长值固定返回 400。六组均要求生成和 refresh 请求为零，合法无续接请求仍走原路径。错误不回显客户端 ID，其他 Provider/Surface 不进入此门禁。`PROTOCOL_EVIDENCE.md` 和 Qoder Provider 文档已同步。

这是 QD-N3 的**中间安全门槛**，不是完整续接：当前任何非空 ID 都会被拒绝；后续仍须按 §18.302 实现有界响应 ID→COSY 会话、工具结果配对、成功终态发布与身份代际隔离，并以首轮→第二轮回环证明真实续接。出站 COSY wire、三 rail 单绑定、401 预算、唯一终态+EOF 与 receipt schema 不变；真实账号验收在本 goal 外且 `live_pending` 不提升。没有做 UI 自动化或部署测试，本轮不提交、不推送。

验证收口：未知续接 ID 的实际 HTTP 回环在旧实现先返回 200；预检后六组 rail × 流式矩阵全部 409 且生成/refresh 为零，畸形 ID 单测也通过。Qoder 定向 Rust 113/113；`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 全量为 library 3602 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1。冻结 Qoder reference-delta `--check-sources`、Qoder CLI oracle、Server Provider 合同/coverage/UI matrix、transform coverage、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings` 与差分空白检查均通过。此前同一工作树的 Qoder 私有 receipt 离线夹具 281/281 不覆盖新增续接语义；没有运行真实账号、UI 自动化或部署测试。QD-N3 完整续接与 TokenRouter 当前 HEAD 全量差分补审仍待继续。

### 18.304 Qoder 续接别名缓存与并发所有权准备

2026-10-14，继续只处理 Qoder。目标新增尚未接入生产转发的响应别名缓存，以当前绑定 runtime scope、Share、用户命名空间、精确模型键与精确响应 ID 组成摘要键；scope 已包含 Provider revision/runtime、Account 身份与代际/实例、站点和凭据 rail。缓存限制为 2 小时、1024 项、8 MiB 总量和 64 KiB 单项，超限拒绝或淘汰最旧项；保存 COSY session ID、system 指纹、工具名及待配对调用。新增原子领取操作，同一别名即使并发也只能由一个续接请求取得，避免两次追加同一 COSY 会话。作用域、过期、冲突、容量、淘汰及并发领取三项单元测试均已通过。当前没有把该缓存接入 Qoder 请求/响应链路，原 `previous_response_id` 409 门禁仍生效，不宣称 QD-N3 完成。

复核首选参考当前已提交 `conversation.go`：Responses 续接可只提交新增 input，省略的 system/tools 从既有会话继承；函数结果必须与明确的调用配对。目标接入时需要把上一响应的未完成调用和本次输出配对，验证显式 system/tools 与缓存状态一致，并确保流式只在真实 EOF、唯一终态和下游完成后发布别名。参考 `871edf920` 的流式脱离客户端取消只用于有界尾部收集，本 Server 不应直接照搬其上下文所有权；断连结算与当前 HEAD 其他 Qoder/gateway 差分仍需后续补审。真实账号验收属于本 goal 外的独立步骤，三条 rail 保持 `live_pending`。

别名不能使用厂商重复或缺省的 Chat ID。目标现只在 Qoder Codex Responses 路由为每次请求生成随机下游 ID，并在 SSE decoder 规范化时统一覆写 Chat chunk 的 ID；非流聚合与流式 Responses 终态由同一 ID 派生，其他 Surface 不变。固定上游始终返回 `chat-qoder-fixture` 的三 rail × 流/非流回环，各连续两次请求均得到不同的 `resp_` ID，且不泄漏上游 fixture ID。该步骤仅建立可发布别名所需的唯一响应身份；尚未发布任何别名或接受续接请求。新增回环 1/1、`cargo fmt --check` 和先前的 `cargo check -q` 通过；完整续接仍待实施，定向/全仓验证须在接线后重新运行。

本轮离线验证收口：Qoder 定向 Rust library 117/117、API 合同筛选 1/1；完整 `CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 为 library 3606 passed / 1 ignored、API 合同 124/124、另两个集成目标各 1/1。冻结 Qoder reference-delta `--check-sources`、文档索引、Provider coverage、`cargo fmt --check`、严格 Clippy `--all-targets -D warnings` 和差分空白检查通过。未运行真实账号、UI 自动化或部署测试；未提交、未推送。QD-N3 完整续接与当前 HEAD 全量差分补审仍未完成，最终整体 review 也未完成。

### 18.305 Qoder Responses 两轮续接接线与隔离回归

2026-10-14，继续只处理 Qoder。已将 §18.304 的有界响应别名缓存接入 Codex Responses 转发：保留转换前原始 `previous_response_id`，只读取得当前账号已有的 session/catalog，按 runtime、Share、用户、上游模型键和精确响应 ID 查找，再核对原始下游模型名。首次请求使用独立 COSY conversation session；命中别名的第二轮只追加本轮 input，省略的 system/tools 从缓存继承，显式改变指纹、孤儿工具结果或与上一轮调用不匹配的 ID 在发网前拒绝。准备成功后原子领取，重复或并发续接不能向同一 session 追加两次；未知、过期、账号代际/凭据、Share、用户及模型漂移均失败关闭。非流在完成响应通过校验后发布新 ID；流式在唯一终态、上游 EOF 和下游继续消费完成事件后才发布，取消或失败不发布。不改 COSY endpoint/header/签名、三 rail 单账号绑定、同账号 401 预算或 receipt schema。

测试夹具中含 `finish_reason=tool_calls` 的帧后原又发送第二个 `[DONE]`，这违反现有唯一终态规则；删除重复终态后，工具结果续接回环通过。作用域回归还暴露单模型策略把不同客户端模型名映射到同一上游键时可误用别名；现在缓存同时保存原始下游模型名并在发网前精确核对，不放宽隔离断言。Global OAuth、CN OAuth、Global PAT × 流/非流的首轮→次轮、工具结果、取消、Share/用户/模型/账号漂移以及并发领取共六项 Responses 定向测试全通过；Qoder Rust 定向 library 123/123、API 合同筛选 1/1。完整 `CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 为 library 3612 passed / 1 ignored、API 合同 124/124、另两个集成目标各 1/1。`PROTOCOL_EVIDENCE.md` 与 Qoder Provider 文档已按当前行为更新，生成类 coverage 未手改。

补充验证：冻结 Qoder reference-delta `--check-sources`、Qoder 私有 receipt 离线夹具 281/281、`cargo fmt --check`、`cargo check --tests -q`、严格 Clippy `--all-targets -D warnings`、Provider coverage、文档索引和差分空白检查通过。这是 QD-N3 的离线实现证据，不是厂商验收或 Qoder 类别收口：TokenRouter 当前 HEAD 剩余 committed-source 差分、断连结算与最终整体 review 仍待核对。真实账号 receipt 采集明确不属于本 goal，三 rail 继续 `live_pending`，不做 UI 自动化或部署测试。本轮未提交、未推送。

### 18.306 Qoder 候选终态、上游 EOF 与断连账务

2026-10-14，继续只处理 Qoder。只读核对 `TokenRouter@c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的 Qoder `execute.go` 与 `871edf920` 的服务端 abort／客户端取消分离：参考会在客户端断连后受限地继续收集上游 usage，但它持有独立 Key 租约与后台执行所有权，本 Server 不直接移植其 detached context。目标 Qoder decoder 虽然直到上游 EOF 才承认成功，原流转换器却能把候选 `finish_reason` 提前转换成 Claude `message_stop` 或 Codex `response.completed`；账务收尾又排在最终 `yield` 后。固定回环旧实现中，Global OAuth Claude 客户端取到 `message_stop` 立即断连被误记为 `client_cancelled`；候选终态后迟到错误还可能在成功事件已外发后才暴露。

现仅在 Qoder 流式转发边界暂存候选终态开始的下游输出，正文继续按原规则流出；暂存额外纳入请求内存预算。只有唯一终态、上游 EOF、runtime 身份与转换收尾都通过，才交付暂存的完成事件，并在最终 chunk 交付前完成成功账务。终态前断连仍由 guard 记中断，尾部错误／超时／预算不足仍失败关闭。Codex Responses 别名继续等客户端在完成事件后继续消费才发布：最终 chunk 到手即断连可算已完成，但该 ID 不可续接。Global OAuth、CN OAuth、Global PAT × Claude/Codex 的最终 chunk 断连和候选终态后错误两组新回环均通过；Qoder Rust 定向 library 125/125、API 合同筛选 1/1。共享 Claude 错误帧在 `event: error` 后仍发送语法性的 `message_stop`，测试要求它只出现在错误之后且成功 `message_delta` 不得提前外发，不把该错误收尾当成成功。COSY 出站 wire、401 预算、Account/Share 绑定、其他 Provider 和 receipt schema 均不变。

同一已提交对象区间的 Qoder 目录差分复核：`5ec14856e` 删除静态目录隐式白名单、`3bb44a6d3` 从较新模型目录注入能力已按 §18.302 保守分类；`4440740a5` 只新增自报模型不应从合成 alias 推断的回归，目标没有可信自报字段；`23e7fdbe4` 移除未使用的非流空白 keepalive；`4b00ccbc5` 迁移测试和不适合本 Server 的跨 Provider 切换入口；`eab81c2c4` 的逆序遍历、map 复制和 min/max 等价改写，以及其余注释、声明顺序、命名整理均不提供新 Qoder wire 证据。相关 gateway Qoder 命名与模型映射增量仍需按入口补审，不能把本段写成当前 HEAD 全量收口。真实账号 receipt 明确在本 goal 外，三 rail 保持 `live_pending`；最终整体 review 未完成，不做 UI 自动化或部署测试，本轮不提交、不推送。

### 18.307 Qoder 完成事件到手即续接的发布边界

2026-10-14，继续只处理 Qoder。§18.306 的初版把 Codex Responses 别名发布排在最终 `yield` 之后；客户端按事件流常规做法在收到 `response.completed` 后立即开始下一轮、没有额外 poll HTTP EOF 时，三 rail 固定回环的首个 Global OAuth 样本先返回 409，说明“已完成却不能续接”。现在只在 Qoder 的上游唯一终态、EOF、runtime 身份、输出转换和待配对工具调用全部通过后，于最终事件交付前发布下一别名；发布失败先记流式失败并发送失败事件，不向客户端交付 `response.completed`。成功账务也在同一最终交付边界提交。终态前取消不会运行发布，终态后的立即续接可命中同账号原 COSY session；不复制 TokenRouter 的账号池或客户端断连后后台推理所有权。先前 §18.305～18.306 所记“完成事件后额外 poll 才发布”的时序已由本节取代，三 rail 的真实账号验收仍在本 goal 外且维持 `live_pending`。

最新代码验证：`cargo fmt --check`、严格 `cargo clippy --all-targets -- -D warnings`、冻结 Qoder reference-delta `--check-sources` 与 `git diff --check` 通过；`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q qoder` 为 library 125/125、API 合同筛选 1/1。此前同一实现的完整 Rust library 3614 passed / 1 ignored、API 合同 124/124 与另外两个集成目标各 1/1，不能以本次定向复跑替代下一轮整体验证。没有运行真实账号、UI 自动化或部署测试。

### 18.308 Qoder 首选参考的 gateway 入口补审

2026-10-14，继续只处理 Qoder。只读按已提交对象比较 `TokenRouter@6d676f1d100e11f2f068949f5779e960c862079c..c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的 Qoder 专属 app、gateway/httpapi、gateway/provider、gateway/text 入口，并与目标 `src/proxy/providers/qoder/mod.rs`、`src/proxy/qoder_runtime.rs` 及 `src/proxy/forwarder.rs` 对照。参考当前工作树仍是未暂存 ` D AGENTS.md`，HEAD 中旧文件仍在；本次只读补审不把工作树删除冒充提交态事实，恢复文件时重核指令。

直接影响 Qoder 的差分可归为三类。`3bb44a6d3` 将统一模型目录的属性解析接到 Qoder runtime，并可按管理员配置改变上下文及思考配置；它依赖参考的较新 1.24.2 模型数据，目标则坚持冻结的 1.1.32 CLI oracle、绑定账号实时启用目录和精确 `model_config`，不能因参考新增属性而扩大能力。`5ec14856e`、`d196e2371` 和 `68e47296f` 调整默认目录、已配置模型扫描及模型映射 API；目标已有绑定账号的权威空目录门禁与显式 alias 解析，未发现需据此放宽的 Qoder 路由。`a3d7407d3` 给参考的异步完成记录任务包装请求上下文；目标在 Qoder 成功终态交付前完成本地账务决策，不迁入参考的异步 worker/Key 租约。参考 HTTP 入口的模型属性错误展示与 `QoderRequestMetadata` 提取、请求生命周期导出命名、`errors.AsType`、Endpoint 同型化、测试迁移和大量注释重排，不形成新的 COSY wire 或本 Server 的单账号能力证据。`593de6d11` 的 Qoder 图片计价回退是参考商业计费路径，不可移植为目标的 Qoder 媒体能力。

这完成了上述 Qoder 命名入口的直接行为差分分类，**不是**整个 gateway 共享执行、完成与错误策略的 HEAD 差分收口；后续需只挑实际可达 Qoder 的共享路径核对，再决定是否产生新反例或关闭类别。§18.305～18.307 的 Responses 续接和终态修复仍以目标 HTTP 回环为证，不由参考重构推定真实厂商接受性。真实账号验收在本 goal 外，Global OAuth、Global PAT、CN OAuth 均保持 `live_pending`；最终整体 review 未完成，不做 UI 自动化或部署测试。本轮未提交、未推送。

### 18.309 Qoder 可达共享路径与旧审计缺口

2026-10-14，继续只处理 Qoder。按当前 `TokenRouter@c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的调用链，Chat 进入 `gateway/qoder.go`，兼容 Messages/Responses 进入 `gateway/text/qoder_compatible.go`；实际推理均进 `upstream/qoder/execute.go`，再到同目录的独立 SSE/协议转换。通用 `gateway/text/responses.go` 与 `gateway/forward/conversion_sse.go` 的 OpenAI/Messages 修复不是这条 Qoder 输出路径，不能把其宽松终态或共享转换器当成 Qoder 行为。完成记录可达 `gateway/provider/completion_snapshot.go`、`gateway/completion/{record,usage_writer}.go`；`2211057d3` 将显示请求 ID 与结算去重键拆开并增加请求详情关联，`4440740a5` 新增上游自报模型投影，`a3d7407d3` 调整异步记录上下文/请求详情更新，`593de6d11` 保留缺价媒体计量单位。这些是参考的多 Key 商业记录语义；目标没有可信 Qoder 自报模型或相同结算组件，现有 Qoder 成功/中断记账及单 Share/Account 守卫须以本仓库回环和整体 review 验证，不能因参考重构直接迁入。`871edf920` 的断连后有界尾部收集已在 §18.306 按所有权边界处理。

复查 §17.9 还发现**更早** `TokenRouter@7faf9469bc6957716923b5b4a98665c0fb9715e0..6d676f1d100e11f2f068949f5779e960c862079c` 当时受旧技能门禁而未审，不能仅凭 §18.302～18.308 的 `6d676f1..c25dbf0` 补审宣告当前 HEAD 全量收口。该旧区间主要把旧 `service/qoder_gateway_service.go` 拆到 `upstream/qoder` 与原生 gateway；Git rename 对比显示 `client.go` 96%、`models.go` 98%、`oauth.go` 97%、`site.go` 97% 保留。已逐项复核这些迁移文件：COSY client 删除未使用的无 context 包装与单事件便捷包装，站点/模型改注释，OAuth 的 PKCE 由同样的 32 随机字节、无填充 base64url 和 SHA-256 challenge 改为共用 helper，未见相应 wire 值变化。`f230af1db`、`d7ae0aa53` 在 Qoder 入口改分组/商业价格及额度投影，不是单账号 COSY wire。旧 service 的 conversation、payload、stream converter 与新模块之间仍有大量跨文件搬迁，后续要按会话、请求构造、SSE/错误和终态四条可达路径补完语义对照；本段只是把遗漏区间重新纳入审计，并未给出全量 no-wire 结论。

旧区间首批跨文件逐项对照：旧 `planWithOptions` 与新 `QoderConversationStore.PlanWithOptions` 均按 system/tools/message 指纹判断前缀或追加、在接受后暂存且仅回滚本次版本；旧 Responses 解析与新 `ParseQoderResponsesPayload` 均保留 `previous_response_id`、生成响应 ID 并把新增 input 交给会话规划。这说明 §18.302 发现的目标续接缺口并非只由较新 HEAD 创造。旧 `streamQoderEvents` 与新 `StreamQoderEvents` 在处理 `IsDone` 时都提前返回，未等待真实上游 EOF；目标已实施的唯一终态+EOF 门槛因此必须继续独立保留，不能以两版参考都接受作为放宽理由。旧 handler 的 401/403 可刷新、`112`/`115` 不刷新与 429/5xx 可切换分类，在新 `MayRefreshAttempt`/`MaySwitchCompatibleAttempt` 中也保留；目标仍只允许绑定账号内恢复，不迁移参考的跨 Provider 切换。以上是已核对的核心路径，不等于旧区间每个输出/错误分支已收口。

本项只读检查，没有修改生产代码，也没有以外仓作为构建或测试输入；真实账号、UI 自动化和部署测试均未运行。三条 Qoder rail 保持 `live_pending`，最终整体 review 未完成。

### 18.310 Qoder 参考门禁与流式失败的部分用量

2026-10-14，核对当前文件系统：`Qoder/AGENTS.md` 仍推荐优先查看 TokenRouter；该参考仓根目录 `AGENTS.md` 在工作树中是未暂存删除（` D AGENTS.md`），Qoder 源码路径下没有其他适用的 `AGENTS.md`。故旧文件要求先加载 `project-doc` 的门禁不再限制**当前工作树**的只读协议研究；HEAD 仍含旧文件，恢复时必须重核。历史 §17.9 的阻断记录不改写。目标仓 `AGENTS.md` 的外仓只读证据、协议文档核对等约束继续适用，不能把门禁解除解读为参考仓可参与构建或运行。

旧 `service/qoder_gateway_service.go` 的 Chat、Claude、Responses 三种流式 writer 在已观察到 usage 后发生读取／转换错误时返回 `nil, err`；新 `upstream/qoder/stream_conversion.go` 三种 writer 将 `HasUsage` 记入结果，并在错误时通过 `qoderPartialStreamResult` 保留“已有输出且有 usage”的部分结果。这是迁移区间的实质性失败账务改进，不是新的 COSY 出站 wire。目标 `finish_qoder_stream_failure` 已从累计器取出已观察 usage，以失败状态写 usage log、Share 调用和 Provider 结果。现有候选终态后迟到上游错误回环补充断言：Global OAuth、CN OAuth、Global PAT × Claude/Codex 六组均保留 `2/1/3` input/output/total tokens，`stream_status=upstream_error` 且 outcome 非成功，也未向下游交付 `response.completed`；定向 `cargo test -q qoder_late_error_after_terminal_never_exposes_success_before_failure` 为 1/1。此次只强化测试和证据，无生产代码改动；读失败等其他分支以及旧迁移区间的会话／请求／终态差分仍待补审，不宣告 Qoder 类别收口。

真实账号验收不属于本 goal；Global OAuth、Global PAT、CN OAuth 继续 `live_pending`。最终整体 review 未完成，不做 UI 自动化或部署测试，本轮不提交、不推送。

### 18.311 Qoder 迁移区间的会话／请求路径与上游读取失败回环

2026-10-14，继续只处理 Qoder。只读对照 TokenRouter 已提交 `7faf9469bc6957716923b5b4a98665c0fb9715e0..6d676f1d100e11f2f068949f5779e960c862079c` 的旧 `service/qoder_gateway_service.go` 与迁移后的 `upstream/qoder/{conversation,payload,execute,stream_conversion}.go`。`planWithOptions` 到 `PlanWithOptions` 保留 system/tools/message 指纹、Responses 只追加 input 的续接、接受状态提交和失败回滚；`parseQoderResponsesPayload` 到 `ParseQoderResponsesPayload` 仍读 `previous_response_id`。请求组装保留独立 request/session/request-set ID、`chat_record_id=request_id`、站点版本、模型配置、system/tools、消息和最后文本块的 ephemeral cache-control；迁移本身未显示需在目标改写的 COSY 请求字段。会话键从参考旧版的 account-scoped 改为新版 provider-scoped，属于参考多 Key 架构的作用域变化；目标继续以绑定 Account 身份/代际、Provider runtime、Share、用户、站点和 rail 做更严格隔离，不把参考作用域缩小移植进来。

流式 writer 的已观测部分用量改进已在 §18.310 分类。为补其“网络读取失败”分支，在目标 Qoder HTTP 回环中加入上游先发正文、再发权威 `2/1/3` usage、最后中断读取且不发终态的夹具；首次夹具错误过早发生于响应建立前，延迟第三帧后才命中预期的已提交流路径。Global OAuth、CN OAuth、Global PAT × Claude/Codex/Gemini 九组均得到部分正文，usage log 保留 `input=2/output=1/total=3`、`stream_status=upstream_error`、outcome 非成功，只发出一次生成请求且不触发 refresh；Codex 均未交付 `response.completed`。Codex 三 rail 还用已交付的 `response.created` ID 立即请求下一轮，均在发网前返回 409，证明失败流没有发布续接别名。§18.310 的“读失败待补审”由此关闭；此项只新增测试，生产代码、COSY wire 和 receipt schema 不变。`cargo test -q qoder_stream_read_failure_preserves_observed_usage_without_success` 1/1；最终 Qoder Rust 定向 library 126/126、API 合同筛选 1/1；冻结 reference-delta `--check-sources`、文档索引、`cargo fmt --check` 和差分空白检查通过。`PROTOCOL_EVIDENCE.md` 与 Qoder Provider 文档已核对，无新行为需要改写；生成 coverage 未手改。

旧迁移区间的其他输入转换、输出终态及可达错误分支，以及较新区间尚未收口的 gateway 共享路径，仍需逐项完成；以上不证明 Qoder 类别或最终整体 review 完成。真实账号验收在本 goal 外，三条 rail 保持 `live_pending`；不做 UI 自动化或部署测试，本轮不提交、不推送。

### 18.312 Qoder 三种流式 writer 与事件循环的迁移差分

2026-10-14，继续只处理 Qoder。对 `TokenRouter@7faf9469bc695` 旧 `WriteQoderOpenAIStreamResponse`、`WriteQoderAnthropicStreamResponse`、`WriteQoderResponsesStreamResponse` 及 `streamQoderEvents`，与 `TokenRouter@6d676f1d100e11f2f068949f5779e960c862079c` 新 `stream_conversion.go` 的对应函数逐段文本差分。三个 writer 的正文、工具、reasoning、usage 映射和完成事件控制分支保留；除 Gin→OutputContext、兼容协议类型→原生协议类型及函数重命名外，可见的实质增量是显式 `HasUsage` 与错误时 `qoderPartialStreamResult`，已由 §18.310～18.311 的目标回环覆盖。事件循环的扫描、keepalive、`IsDone` 提前返回和错误分支在所比区间只作类型/命名迁移；旧 `pkg/qoder/client.go` 到新 `upstream/qoder/client.go` 的重命名差分也未改 `ParseSSELine`/事件归一化函数主体，仅删未使用的无 context/单事件便捷入口。再看 `6d676f1..c25dbf0` 的 parser 函数差分，`ParseSSELine`、内容块、工具和 usage 事件函数的相应 hunk 是说明注释/字段注释，未据此推导新增 wire。目标继续要求唯一终态及上游 EOF，不继承提前返回。此结论覆盖上述可达 writer、事件循环与 parser 控制逻辑，不替代对协议序列化器、错误提取和其余 gateway 路径的核对。

本节只读对照；没有修改 TokenRouter、目标生产代码或生成文档。真实账号验收仍在本 goal 外，三 rail 保持 `live_pending`；Qoder 类别及最终整体 review 尚未收口，不做 UI 自动化或部署测试，本轮不提交、不推送。

### 18.313 Qoder 三种输入解析与工具消息的迁移差分

2026-10-14，继续只处理 Qoder。逐段对照 `TokenRouter@7faf9469bc695` 旧 `service/qoder_gateway_service.go` 与 `TokenRouter@6d676f1d100e11f2f068949f5779e960c862079c` 新 `upstream/qoder/payload.go`：Chat、Responses、Claude 三种请求解析仍以同名字段取得模型、system、工具、token 上限、thinking、显式 session/prompt cache key 和 metadata；Responses 的 `previous_response_id` 与响应 ID 逻辑保持。三种消息解析、Responses 工具调用/结果配对、Claude 内容块、工具声明/名称映射、最终 COSY message 构造等对应函数的差分是 `apicompat`→原生协议类型、导出命名及调用名迁移，未见新增可移植的单账号 Qoder wire。相关旧/新请求结构体中这些实际读取字段与 JSON tag 也一致。

Claude 请求只借通用 `AnthropicToResponses` 转换后的 `Tools`：桥迁移将采样参数和 reasoning 规则改为 `RequestOptions`，但 Qoder 调用前已清空 `System`/`Messages`，之后只读取 `responsesReq.Tools`；两版 `convertAnthropicToolsToResponses` 的工具转换逻辑保持。较新 `3bb44a6d3` 的 Qoder payload 差分仅让参考模型目录/管理员覆盖改变 context 与 thinking 能力，已在 §18.302 按目标冻结 CLI oracle、实时目录与保守能力门禁分类；`eab81c2c4` 的逆序遍历、`maps.Copy` 是等价整理。目标 `src/proxy/adapters.rs` 先按 Claude/Codex/Gemini Surface 转规范 Chat，再由 `src/proxy/qoder_runtime.rs` 检查工具历史、续接和只支持的内容并组装 COSY payload；它不因参考宽松丢弃多模态内容而放宽失败边界。此项只读对照，没有新增生产代码。

至此旧迁移区间的主要会话、请求组装、输入转换与流式 writer/事件循环已经按可达路径分类；仍需继续核对非流输出聚合、错误提取/结算和较新 HEAD 的相关共享序列化路径，不能凭本节宣布 Qoder 完成。真实账号验收在本 goal 外，三 rail 继续 `live_pending`；最终整体 review 未完成，不做 UI 自动化或部署测试，本轮不提交、不推送。

### 18.314 Qoder 非流输出构造与嵌套错误分类

2026-10-14，继续只处理 Qoder。只读逐段核对 `TokenRouter@7faf9469bc695` 旧 `service/qoder_gateway_service.go` 的 Chat、Claude、Responses 非流处理和 `BuildQoderOpenAICompletion`、`BuildQoderAnthropicMessage`、`BuildQoderResponsesResponseWithID`，与 `6d676f1d100e11f2f068949f5779e960c862079c` 新 `upstream/qoder/{execute,stream_conversion}.go` 的对应逻辑。三路仍先收集 SSE 事件、转换客户端协议，再记录 usage、提交会话；三个构造函数的正文、reasoning、工具、stop reason 和 usage 映射保留，Responses bridge 由通用调用改为显式注入时间/随机源。`6d676f1..c25dbf0c` 在这些构造函数的差分主要是注释；未见新增可移植的 COSY 非流输出 wire。参考事件循环遇 `IsDone` 提前返回，目标的 `aggregate_qoder_nonstream` 则等待上游 EOF、唯一终态及 runtime fence，不能放宽。目标非流转换、别名发布与成功账务仍在最终响应提交前完成。

错误提取的可达差异是参考 `client.go::applyQoderErrorPayload` 可继续深入 `data` 并解析 JSON 字符串 message；其较新 HEAD 差分主要是注释。目标 `QoderUpstreamError::from_status_body` 原只看顶层、一层 `error`/`data`，两层 `data` 中的 `115`、message 和限额重置时间都漏掉，固定反例在旧实现先红。现对 `error`/`data` 最多深入 8 层；至多 64 KiB 的 JSON 字符串 message 只作为缺失 code/reset 的后备。外层非 null code 优先，未知 code 不能被内层 `115` 洗成限额，公开 code 仍只允许 `112`/`115`，message 脱敏和一次同账号 401 预算不变。此项不改 COSY 请求或成功输出，不借参考的多 Key 结算/断连所有权；真实账号验收在本 goal 外，三 rail 继续 `live_pending`。较新 HEAD 可达共享序列化与完整 Qoder 类别收口、最终八类 review 仍待完成，不做 UI 自动化或部署测试，本轮不提交、不推送。

验证：新增固定反例旧实现失败、修复后 1/1；Qoder Rust 筛选为 library 127/127、API 合同 1/1，其他筛选目标 0/0。冻结 Qoder reference-delta `--check-sources`、Server Provider 合同、Provider coverage、UI Provider 静态矩阵、文档索引、`cargo fmt --check`、严格 Clippy `--all-targets -D warnings` 与 `git diff --check` 均通过。本轮没有运行完整 Rust 全量、真实账号、UI 自动化或部署测试；不能以定向通过宣称最终整体 review 完成。

### 18.315 Qoder 当前 HEAD 可达共享执行、输出与完成路径

2026-10-14，继续只处理 Qoder。按 `TokenRouter@c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的实际调用链，原生 Chat 经 `gateway/qoder.go`，兼容 Messages/Responses 经 `gateway/text/qoder_compatible.go` 和 `gateway/httpapi/qoder_compatible_attempt.go`，最终均由 `upstream/qoder/execute.go` 产生客户端协议输出。`gateway/output.go` 把 HTTP 字节提交与可重试的语义输出分开；已服务且有 usage 的失败先结算后结束，不再刷新或切换。`ForwardQoderAttempt` 只有同时满足 `Served`、`HasUsage` 才把失败的部分结果传给完成记录；兼容入口同步捕获结算输入、异步写商业 usage。目标则以单 Share/Account 绑定、下游成功事件交付前的账务提交，以及流式失败时的已观测 usage、失败 outcome 和不重试回环落实自己的所有权，不迁移参考的多 Provider/Key 选择、断连后继续推理或商业计费 worker。

对 `6d676f1d100e11f2f068949f5779e960c862079c..c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的这些可达共享文件逐项核对：`gateway/qoder.go`、`gateway/output.go`、`gateway/text/qoder_compatible.go` 及 HTTP attempt/error presenter 的主体执行、提交和失败分支保持，主要增加注释、声明整理、`errors.AsType` 与 `QoderRequestMetadata` helper 提取。`unified_text_executor.go` 增加模型定价前检并投影自报模型；前者是参考资金准入，后者需要可信上游自报，不授权目标在冻结 CLI oracle 与绑定账号实时目录之外扩大模型能力。`gateway/provider/completion_snapshot.go`、`gateway/completion/{record,usage_writer}.go` 的显示请求 ID／结算去重键拆分、请求详情补录、自报模型及缺价媒体量保留属于参考多 Key 商业记录语义，已在 §18.309 分类；目标没有同构组件或可信 Qoder 自报字段。共享 OpenAI/Messages SSE 转换器仍不在 Qoder 输出调用链上。当前审查未找到新的可移植单账号 COSY wire，目标既有唯一终态+EOF、同账号一次 401 和 QD-N3 Responses 续接保持；本节不声称所有 Qoder 控制面迁移文件或最终八类 review 已完成。

本节只读比较，没有修改生产代码或外仓，也没有运行真实账号、UI 自动化或部署测试。三条 Qoder rail 继续 `live_pending`；本轮未提交、未推送。

### 18.316 Qoder 旧迁移区间的认证与编码原语

2026-10-14，继续只读比较 `TokenRouter@7faf9469bc6957716923b5b4a98665c0fb9715e0` 旧 `backend/internal/pkg/qoder` 与 `6d676f1d100e11f2f068949f5779e960c862079c` 新 `backend/internal/upstream/qoder` 的同名生产文件。`signature.go` 对应文件无文本差分；`auth.go` 仅删除不带 context 的 PAT/refresh 便捷包装；`encoding.go` 删除字符串便捷包装、测试专用 panic helper 和虚置 import 引用，`Encode`/`Decode`/`EncodeJSON` 主体不变；`session.go` 删除默认 Global/无显式 profile 的包装，站点 profile、COSY payload 与 AES 主体不变。`auth_status.go` 的 `AccountID`→`ProviderID`、`IsSubAccount`→`IsSubProvider` 是 Go 字段名迁移，JSON tag 仍分别为 `accountId`、`isSubAccount`，identity 取值仍用同一响应字段。旧 `local_auth.go` 搬至 `localauth` 子包，解密/JSON 读取仍相同，删除自动装配本地桌面身份的 helper；本 Server 的三 rail 由受控 Account 导入/device flow/PAT 建立，不应把本地桌面文件读取引入服务端。以上不形成新的 origin、签名、凭据轮换或 COSY wire 修改理由。

新拆出的 `authorization.go`、`credential_builder.go`、`refresh_exchange.go` 与 `quota_usage.go` 对应旧 service/provider 控制面逻辑，仍需逐项追踪其实际调用与旧实现，特别是 CN 身份完成、持久化后 generation fence 和 quota 显式零的业务边界。当前只是关闭上述同名原语文件的迁移差分，不宣布 Qoder 类别或最终八类 review 收口。真实账号验收在本 goal 外，Global OAuth、Global PAT、CN OAuth 继续 `live_pending`；不做 UI 自动化或部署测试，本轮未提交、未推送。

### 18.317 Qoder 旧迁移区间的 OAuth 完成与凭据交换

2026-10-14，继续只读追踪旧 `service/qoder_oauth_service.go::completeSessionOnce` 与新 `upstream/qoder/authorization.go::CompleteAuthorization`：CN 仍先验证 token、取 userinfo、调用 CN status 完成身份，并以 OpenAPI user ID 而非可能不同的 COSY account ID 查询可选组织信息；Global 仍允许 userinfo 查询失败后用 token response user ID 构造身份，并只写固定脱敏 warning。两版 token-info 组装均裁剪凭据、保留 site/refresh mode 和非零 RFC3339 expiry；这段搬迁没有改变 OAuth 站点、PKCE 或身份主数据优先级。目标 Qoder 的站点/rail 显式绑定、CN 完整身份闭环和刷新 receipt durable commit 门槛比参考更严格，不因搬迁回退。

旧 `service/qoder_token_refresher.go::Refresh/refreshIdentity/qoderTokenInfoCredentials` 与新 `upstream/qoder/refresh_exchange.go::Refresh/refreshIdentity/TokenInfoCredentials` 对应分支也保留：PAT 按原站点重建 job-token 身份；CN20 只走 CN OpenAPI refresh/userinfo/status；导入的 CN COSY 和 Global COSY 各用原 rail，缺 refresh token、缺 machine ID 或缺返回 security token 时拒绝；新身份字段裁剪，未轮换的 refresh token 仍可从原凭据保留。参考把 exchange 与保存者拆开，新 `provider/provider/qoder_refresh.go` 只合并凭据并返回 map，持久化由调用方的 refresh coordinator 执行；不能把仅 exchange 成功当作目标 Account 代际已安全轮换。目标的 pending receipt／generation fence 需继续以本仓库离线回环验收，不以参考多 Key cache 作为实现模板。当前没有发现需改 COSY/OAuth 出站 wire 的新差分；quota 迁移、持久化后缓存作废与当前 HEAD 控制面增量仍待进一步核对，Qoder 类别和最终 review 未完成。真实账号验收在本 goal 外，三 rail 仍 `live_pending`；不做 UI 自动化或部署测试，本轮未提交、未推送。

### 18.318 Qoder quota 显式零与服务端调度边界

2026-10-14，只读对照旧 `service/account_usage_service.go` 中的 `qoderQuotaProgressRaw.UnmarshalJSON`、`qoderJSONHasAnyField` 与新 `upstream/qoder/quota_usage.go`：两版都用独立 presence 位区分字段缺失、`null` 与数值零／布尔 false；`remaining:0` 不会被 `total-used` 覆盖，`available:false` 不会被推断为 true。新版主要把原有 quota wire 结构和归一化搬到 Qoder upstream 包，并补充说明性注释，未见新的 quota 字段选择或实际可用性证据。目标 `src/clients/oauth/quota.rs::parse_qoder_quota_update` 同样保留显式零，但不把个人账户的全零、缺少完整余额证据直接判为耗尽；只在各已存在 bucket 的剩余额可信且无正余额，并有 `isQuotaExceeded=true` 或正容量证据时设置限额。它还拒绝冲突 alias、负数和超容量，维持本 Server 单 Account 调度的保守策略。参考额度展示和多 Provider 冷却不授权目标改变站点／rail 或跳过实时目录；本项无生产改动。

仍需核对旧迁移区间的持久化／缓存作废调用链、当前 HEAD 控制面增量，以及 Qoder 收口前的完整目标回归；真实账号验收在本 goal 外，三 rail `live_pending`，最终八类 review 未完成。不做 UI 自动化或部署测试，本轮未提交、未推送。

### 18.319 Qoder 会话缓存、刷新持久化后的失效与停机所有权

2026-10-14，只读追踪 `TokenRouter@7faf9469bc695` 旧 `service/qoder_token_provider.go`、`qoder_gateway_service.go`、`token_refresh_service.go`、`token_cache_invalidator.go`，以及当前 `c25dbf0c` 的 `provider/qoder_sessions.go`、`provider/provider/{qoder_tokens,qoder_request_refresh}.go`、`provider/{refresh_attempts,refresh_post_actions,token_cache_invalidator}.go`。旧、新会话缓存都以完整凭据摘要和代际隔离 singleflight：较旧 `_token_version`／更新时间快照不能反向覆盖已观察到的新身份，`InvalidateAccount`→`InvalidateProvider` 接受权威快照时提升代际并淘汰旧 session，在途构建写缓存前复核代际。请求失败触发的刷新也保留“持锁则等数据库出现不同凭据摘要；有刷新结果用其快照，否则重读；无可用重读时仅作 ID 失效”的分支。后台刷新成功或已持久化但尝试超时／请求取消时，两版都先做缓存失效再同步调度快照；当前版将此序列拆为 `RefreshPostActions` 与组合 invalidator。新缓存另外由 activity 管理共享构建的停机取消／等待，旧版只有独立的 90 秒构建预算。这属于参考仓共享异步任务所有权改进，不等于 Qoder 上游协议发生变化。

目标 `src/state.rs::prepare_qoder_runtime` 与 `src/proxy/qoder_runtime.rs::QoderRuntimeScope` 不共享参考的 Provider-ID 会话缓存：scope 包含 Provider revision/runtime 指纹、绑定 Account 的认证代际、刷新代际、incarnation、站点、rail 和请求身份指纹。OAuth 刷新先经 `commit_native_refresh_success` 的持久化门槛，随后重取 Account；PAT 交换和目录写入前复核当前快照，出站使用和续接也核对 generation/scope。代际变化产生新 scope，旧 session/catalog 不会被新请求命中；旧项按缓存上限回收。目标的刷新 flight 由 `managed_account_refreshes` 管理停机，不采用参考的脱离单请求之共享 Go session 构建。此处未找到需要直接移植的单账号 wire 或缓存语义，亦不能把参考组合 invalidator 复制到目标的状态所有权边界。此项没有修改生产代码；当前 HEAD 其余控制面增量和 Qoder 完整回归仍待收口。

TokenRouter 当前工作树的根 `AGENTS.md` 是未暂存删除（HEAD 仍追踪），仓内没有其他现存 `AGENTS.md`；旧文件的 `project-doc` 门禁不限制本次当前工作树的只读研究，若恢复文件须重核。`Qoder/AGENTS.md` 仍指定 TokenRouter 为首选；目标仓 `AGENTS.md` 的外仓只读证据及文档核对约束仍适用。真实账号验收不属于本 goal，Global OAuth、Global PAT、CN OAuth 均保持 `live_pending`；不做 UI 自动化或部署测试，本轮不提交、不推送。

### 18.320 Qoder 当前 HEAD 控制面增量复核

2026-10-14，继续只读审查 `TokenRouter@6d676f1d100e11f2f068949f5779e960c862079c..c25dbf0cfa5a806460ed0fbc5c43a55797148537` 中 Qoder 可达的授权、刷新、会话、额度、模型和管理探针文件。`provider/qoder_refresh_identity.go` 删除时，筛选 `pat`、COSY token、machine、用户/组织、site 与 refresh mode 的 `QoderRefreshCredentialsHash` 原样搬入 `provider/qoder_refresh.go`；请求失败刷新与锁竞争等待两版都使用该摘要，不把 `_token_version`、过期时间或模型映射变化误判为已换认证身份。目标不沿用其多进程 DB 锁等待：当前同账号强制刷新按 Account 认证/刷新代际 singleflight、持久化门槛与 runtime scope 复核；PAT 401 仅淘汰当前 scope 后重新换 job token，同一请求最多恢复一次。目标的凭据指纹还覆盖请求实际使用的 profile、machine、endpoint 和 relogin 状态，非认证元数据变更只会使旧 runtime 失效，不会跨账号或跨站点重试。

参考的 `provider/qoder_authorization.go`、`provider/provider/qoder_authorization.go`、`upstream/qoder/{authorization,oauth,auth_status,credential_builder,refresh_exchange,site,session}.go` 该区间可达主体保持授权 proof、CN/Global 身份与签名/站点选择；改动主要是注释、声明位置及 `WaitGroup.Go` 等等价写法。`provider/provider/qoder_usage.go` 的 `errors.As`→`errors.AsType`、`provider/qoder_validation.go` 的直接返回验证错误、测试探针映射签名改为直接裁剪输入，对已核对的 Qoder 业务分支不构成新 wire。模型目录的 `5ec14856e` 删除静态空白名单、`3bb44a6d3` 注入目录属性至 context/thinking 已在 §18.302 与 §18.306 分类；本 Server 仍按官方 CLI oracle、绑定账号实时目录和保守能力规则判定，不能用参考管理员覆盖或模型广场取代账号授权。`4440740a5` 自报模型记录不提供可信 Qoder 上游字段，已在 §18.315 排除。gateway 执行与完成路径见 §18.308、§18.315；输出、错误提取、刷新持久化和 session fence 见 §18.310～18.319。至此所列 Qoder 可达主体的旧迁移与当前 HEAD 增量均已分类，未发现本节还需迁移的新单账号 COSY 出站字段；这不是厂商真实接受性证明。

本节没有修改生产代码或外仓。冻结的 schema-v2 reference-delta `--check-sources` 只复核其历史 `7faf9469` 对象，不自动证明本节的 `c25dbf0c` 补审；最新补审以本节及前述逐路径记录为证。Qoder 定向 Rust library 127/127、API 合同筛选 1/1，私有 receipt 离线夹具 281/281、官方 CLI oracle、历史 reference-delta `--check-sources` 和文档索引通过；完整目标回归仍在进行，Qoder 类别暂不宣告收口。真实账号验收不属于本 goal，三 rail 继续 `live_pending`；不做 UI 自动化或部署测试，本轮不提交、不推送。

### 18.321 Qoder 离线实施收口与最终整体 review 入口

2026-10-14，§18.320 后没有再发现需改动的 Qoder 单账号出站 wire；本轮把 Qoder 当前 HEAD 只读补审及历史 `project-doc` 门禁已解除的**当前工作树**状态写入 Provider coverage 生成源，并重新生成 `docs/provider/coverage.md`。该文档现在明确区分 2026-09-28 的不可变 `unreviewed_skill_gate` 历史、2026-10-14 的逐路径补审与三 rail 仍缺真实 receipt；旧 oracle 的 `65` Rust / `7` 私有验收夹具计数也明确标作冻结基线，当前回归数字以本节为准。`docs/provider/qoder-cosy.md` 同步说明历史与当前状态，不修改旧 reference-delta observation。Phase 0 writer inventory 通过生成脚本只刷新当前 `src/api/providers.rs`、`src/state.rs` 的源码摘要；外仓未写入，也未成为构建或运行输入。

验证：Qoder 定向 Rust library 127/127、API 合同 1/1；完整 Rust library 3616 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Qoder 私有 receipt 离线测试 281/281；全量后端 Node audit/smoke 首轮 1905/1909，有 4 项失败且首轮输出未保留可辨失败名，不能确认原因；在文档生成与 Rust 编译停止后的独立完整复跑为 1909/1909，保留首轮波动供最终 review 检查。`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、官方 CLI oracle、冻结 reference-delta `--check-sources`、Provider/Phase 0/coverage/UI 静态矩阵、transform coverage、文档索引、差分空白检查以及本地 server health/setup/auth/Provider/Share smoke 通过。完整 `scripts/static-checks.sh` 含 Web UI 测试，按禁令未运行；没有部署测试。

当前工作树级 Qoder 离线实施和可达参考差分已收口，可进入**八类合并后的整体 review**；这不等于已完成该 review，也不把未提交工作树冒充 committed-target observation。冻结 Qoder reference-delta 的 `--check-sources` 仍只校验历史对象，当前 HEAD 结论以 §18.302～18.320 的只读路径记录为证。真实账号验收明确在本 goal 外，Global OAuth、Global PAT、CN OAuth 三 rail 保持 `live_pending`，不据此阻断本 goal 的离线实施收口；本轮未提交、未推送。

### 18.322 八类整体 review：证据一致性与 Claude 续线程缺口

2026-10-14，开始八类合并后的整体 review。第 0～16 节和 §18.1～18.4 是其各自冻结日期的历史判定，不再以其中“Qoder 当前仍受 `project-doc` 阻断”或“真实 rail 未完成则整个计划不能结束”的旧语句判定**本 goal** 的当前状态。用户已明确真实账号验收在本 goal 外；所有 rail 继续按各自合同 `live_pending`，不伪造 receipt、不提升 `live_verified`。本 goal 仍须完成离线实施、相称回归和八类最终 review；UI 自动化及部署测试继续禁止。

| 复核面 | 当前工作树证据与判断 |
| --- | --- |
| 八类参考差分 | 八个自包含 `audit-*-reference-delta.mjs` 默认审计均通过。Qoder 原结构化证据只有 9 月门禁，本轮在 `qoder-reference-delta.json` **追加** `QODER-CURRENT-HEAD-READ-ONLY-2026-10-14`，保留旧记录和所有 schema-v1 字段不变；脚本以独立不可变摘要校验两条记录。显式 `--check-sources` 已验证旧冻结文件及 `7faf9469`、`6d676f1`、`c25dbf0` 三棵 tree；默认审计不访问外仓。tree 校验不替代 §18.302～18.320 的逐路径结论，也不证明目标提交态。 |
| 产品与状态边界 | Server product/web boundary、source provenance、SQLite authority、Provider conformance、coding-plan/web-session registry、Provider coverage、UI Provider **静态矩阵**及文档索引审计均通过。当前 `src/proxy/claude_thread_alias.rs` 虽未跟踪，但由 `mod.rs`、`state.rs` 和转发热路径实际接入，Rust 编译/测试覆盖它；未跟踪状态必须在交付时保留，不能误删。 |
| 当前离线回归 | 独立完整后端 Node audit/smoke 为 1909/1909。新增 Claude 反例旧实现先红、修后 1/1；完整 Rust library 3617 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1。`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、Qoder `--check-sources`、Provider/Phase 0 合同、CLI oracle、transform coverage、生成 coverage 与差分空白检查通过。此前 §18.321 的首轮 Node 波动记录仍保留，本轮独立完整复跑通过不抹去历史。 |
| Claude review 发现 | 无声明续线程从缓存恢复别名时，`restore_request_names` 漏掉 `tool_result.content[]` 和 `tool_search_tool_result.content.tool_references[]` 中的协议 `tool_reference`。固定反例旧实现仍发送非法原名；现复用普通 Claude 请求路径的嵌套引用改写，仅改协议位置，不碰普通文本和工具 `input`。未改变线程缓存作用域、出站 endpoint/header、固定绑定或真实状态。 |

本节是整体 review 的**第一批**证据和修复，不宣告八类最终 review 已完成。仍须逐项核对八类最新热路径与 §16/§18.4 中可离线退出条件、交叉 Provider 身份/预算/终态不变量及文档/合同覆盖是否充分；尤其需确认未提交工作树的全部现有改动没有被历史提交态观察误代表。真实账号 receipt、UI 自动化和部署测试明确排除，不作为本 goal 的剩余项。本轮不提交、不推送。

本地服务 smoke 首次由 `cargo run` 在 30 秒启动窗口内重新编译而未监听，脚本报连接拒绝；该轮没有形成服务行为结论。随后 `cargo build -q` 预构建完成，同一 smoke 在独立临时配置和 127.0.0.1 端口下复跑通过 health/version、Web fallback、setup、密码/API token 登录、Provider 创建与 Share 创建。没有部署或真实账号请求。

### 18.323 整体 review 的 Claude 续线程部分声明门槛

2026-10-14，继续仅复核 Claude。`claude_thread_alias::thread_mode_from_value` 原只以 `tools` 是否为空决定是否需要上一条 message ID 的别名状态。固定反例让本轮只声明 `mcp__new/name`，历史 `tool_use` 仍引用 `mcp__old/name`：旧单测判 `needs_state=false`，实际 HTTP 回环在无旧状态时发网并返回 200；两层旧行为均先红。相同缺口还覆盖嵌套 `tool_reference`、`tool_choice`、`tool_addition`/`tool_removal` 的工具引用字段和 `server_tool_use`，补充反例旧实现也先红。

现在只在这些精确协议位置引用了**本轮未声明**、且需要非法名称别名或 Claude Code 内建工具大小写归一的名称时，要求前一条消息的精确作用域状态；缺失则本地 404、零上游请求。有完整声明时历史名称与出站声明共用 alias；合法且无需改名的未知历史名称仍走原路径。普通文本和工具参数中的同名字段不参与检测。扫描前先检查原有 128 条工具声明上限，超出时本地 400，避免为异常大声明列表构建临时集合；正常名称先判断是否需要别名，避免重复小写分配。存在旧状态时 `restore_request_names` 对 choice、历史调用、嵌套引用及工具增删共用旧映射。只修 Claude 续线程的发网前完整性，不改 endpoint/header、缓存 TTL/容量/身份作用域、Account/Provider/Share 绑定、其他 Provider 或真实状态。

验证：部分声明的单测与 HTTP 零发网回环修后通过；choice/change 反例修后通过，完整声明的 HTTP 正例同时核对历史名称等于出站声明 alias。最终完整 Rust library 3620 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -D warnings`、Claude reference-delta、自包含 Provider conformance/coverage/UI 静态矩阵、文档索引及差分空白检查通过；本轮未重跑后端 Node 全量，§18.322 的独立 1909/1909 是此前同一工作树脚本版本的结果。`docs/provider/coverage.md` 生成状态与 conformance 等级未改，`PROTOCOL_EVIDENCE.md` 和 Claude Provider 文档已补本地边界。真实账号验收在本 goal 外，五项 Claude operation 仍 `live_pending`；未做 UI 自动化或部署测试，本轮不提交、不推送。八类整体 review 继续进行，不因本节局部通过宣告完成。

### 18.324 整体 review 的 Codex HTTP 回退错误优先级

2026-10-14，继续仅复核 Codex。`relay_codex_http_fallback_stream` 在 HTTP SSE 已向下游发送上游 `error` 后，如果又收到当前 `response.created` ID 的匹配 `response.interrupt`，旧实现仍合成 `response.incomplete` 并返回 `Cancelled`，把已发生的上游失败记为 499/`client_cancelled`。新增本地回环 `codex_http_fallback_error_precedes_matching_interrupt_outcome` 固定该顺序，旧实现先红：期望 `response.failed`，实际得到 `response.incomplete`。

现在仅在精确匹配中断且已有 `last_error` 时，先补发尚未交付的生命周期帧，再合成 `response.failed`，返回 `Completed(SemanticTerminal::Failure)`，沿用外层既有 Provider 失败结算；使用 `last_error.take()`，不复制错误内容。无先到达上游错误的匹配中断仍为 `response.incomplete`、499/`client_cancelled`；未知或不匹配 ID 仍不能取消当前 HTTP 轮次。该修复不改 Codex OAuth 出站 endpoint/header/body、Account/Provider/Share 单绑定、同账号恢复预算或 receipt schema v1/revision 1。离线竞态反例不证明厂商对中断或续接的真实接受性。

验证：Codex HTTP fallback 定向组 23/23；完整 Rust library 3621 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -- -D warnings`、Codex reference-delta 默认审计、Provider conformance/coverage、UI Provider 静态矩阵和文档索引通过；本轮尚未重跑后端 Node 全量，§18.322 的 1909/1909 是此前同一工作树脚本版本的结果。生成 coverage 状态未变，`PROTOCOL_EVIDENCE.md` 与 Codex Provider 文档已补失败优先级。真实账号验收不在本 goal 内，Codex 五项 operation 继续 `live_pending`；TokenRouter 当前工作树删除的根 `AGENTS.md` 不再施加旧 `project-doc` 门禁，但 `Qoder/AGENTS.md` 的首选参考和目标仓外部只读取证约束仍有效。八类整体 review 尚未逐项收口；不做 UI 自动化或部署测试，本轮不提交、不推送。

### 18.325 整体 review 的 Cursor marker-only 工具错误脱敏

2026-10-14，继续仅复核 Cursor。`Cursor/AGENTS.md` 推荐的 `OmniRoute` 当前提交 `443d66996d69d7ecf887fd61158404a3cca1b192` 与冻结 tree `8d1c312f61d115767c9478416cb039750eaf113a` 未变化；22 项未提交 Codex 工作树内容不作 Cursor 证据。冻结 reference-delta 的八条 observation 与 `CUR-N2=reviewed_no_wire_delta` 仍成立。本轮复核目标本地安全边界：Composer marker-only 工具调用没有可续接的 Exec/MCP 元数据，`cursor_delta_events` 的 Text/TurnEnded 和 `flush_cursor_marker_filter` 原将上游生成的工具名插入 `ProxyError.message`，流式入口会把该文字发给客户端。含 `private-marker-name` 的固定反例在旧代码先红；初版测试因 `unwrap_err` 要求成功类型实现 `Debug` 未编译，修正测试写法后才得到生产行为红灯。

现三条拒绝路径统一使用固定的 501 错误，既不猜测 marker 工具身份，也不回显上游名字。Anthropic Messages、OpenAI Chat、OpenAI Responses、Gemini 四种下游错误形状和原有失败关闭保持；不改 OAuth/API Key、Account/Provider/Share 单绑定、AgentService h2/protobuf、session/续接、Provider 选择、用量或 receipt schema v2/revision 2。顺带核对流式每帧 delta 循环：当前 `decode_agent_server_message` 最多从一个 protobuf oneof 帧返回一项 delta，不能把循环形式本身误判为同帧错误后继续发布内容的可达缺陷。离线 marker 反例不证明厂商实际发送该名称；两条真实 rail 继续独立 `live_pending`，真实账号采集不在本 goal 内。

验证：固定反例修后覆盖四种入口；Cursor Rust 定向 library 344/344、API 合同筛选 1/1、其他筛选目标 1/1；完整 Rust library 3622 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Cursor 私有 receipt 离线测试 95/95、冻结 reference-delta `--check-sources`、Provider coverage、UI Provider 静态矩阵、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -- -D warnings` 与差分空白检查通过。`docs/provider/coverage.md` 生成状态未变，`PROTOCOL_EVIDENCE.md` 与 Cursor Provider 文档已补本地脱敏边界。本轮未重跑全量后端 Node；§18.322 的 1909/1909 不能冒充本轮结果。八类最终整体 review 尚未完成；没有 UI 自动化、部署测试、真实账号请求、提交或推送。

### 18.326 整体 review 的 Grok SSE ping 显式类型门槛

2026-10-14，继续仅复核 Grok。`Grok/AGENTS.md` 首选的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 及次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 当前提交仍是冻结点；前者干净，后者六项未提交 Codex 工作树内容全部排除。两者没有新的已提交 Grok wire；参考的 Responses 兼容层处理 BOM/私有 Build 控制事件，并不定义目标 direct-XAI 计费 ping 的任意 JSON 形状。`GR-N4` 同账号版本/header 继续 `live_pending_no_wire_change`，不由离线 ping 修复推断上游接受性。

目标 `grok_sse_event_is_ping` 原将非空 JSON 的 `type` 读取为可选字符串：数组、字符串等非对象数据，以及对象中显式 `type=null`/数字，都与缺失 `type` 混同，把整帧改写成 `: ping`。含数组数据的固定反例在旧代码先红，证明不明上游内容会被静默丢弃。现仅接受 JSON 对象；对象的 `type` 只可缺失或精确等于字符串 `ping`，仍先拒绝错误、失败及终态语义。无数据 ping、正常无 `type` 的对象型计费 ping、合法 `type=ping` 保持；非对象与显式非字符串 type 原样透传，并在任意两块分片下保真。主 Responses 语义门禁仍决定其最终接受/拒绝，不把旁路过滤器变成新的成功证明。只改 Grok OAuth Responses SSE 入站过滤，不改出站 endpoint/header/body、HTTP JSON/原生 WS、Account/Provider/Share 单绑定、搜索/质量证据提交、同账号恢复、remote compaction disabled 或 receipt schema v1/revision 1。

验证：四个 ping 专项定向测试通过；Grok Rust library 264/264、API 合同筛选 2/2；完整 Rust library 3622 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Grok 三 operation 私有 receipt 离线测试 159/159、冻结 reference-delta `--check-sources`、Provider coverage/UI 静态矩阵、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -- -D warnings` 与差分空白检查通过；生成 coverage 状态未变。`PROTOCOL_EVIDENCE.md` 与 Grok Provider 文档已补本地过滤边界。本轮没有重跑后端 Node 全量；§18.322 的 1909/1909 不是本轮结果。inference、media、remote_compaction 三项仍各为 `live_pending`，真实账号验收不在本 goal 内。八类最终整体 review 尚未完成；没有 UI 自动化、部署测试、真实账号请求、提交或推送。

### 18.327 整体 review 的 Kiro 歧义工具名称脱敏

2026-10-14，继续仅复核 Kiro。`Kiro/AGENTS.md` 首选且干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` / tree `5d22f040f0e4b0aa775e61547540b3c040293cd8` 仍是冻结提交点。参考 `ToolUseEvent` 的 `name` 是上游事件字段，不能直接成为本 Server 的客户端错误文字。目标 `KiroToolNameRegistry::resolve` 对多个当前声明共享同一子名称、上游只返回该短名称的情况正确拒绝歧义，但旧错误把 `upstream_name` 格式化进 `KIRO_EVENT_STREAM_INVALID`。含 `private-child-name` 的真实 EventStream 非流转换固定反例先红；相同错误也可走流式桥接。

现仅将这条歧义错误改成固定说明，不回显短名称。精确声明名、唯一子名称和多候选拒绝决策不变；流式与非流转换都保持 `KIRO_EVENT_STREAM_INVALID`，流式拒绝后不发送成功 `message_stop`。不改 Kiro/Amazon Q 出站 endpoint/header/body、EventStream 帧/终态/EOF、工具 ID/JSON 聚合、Account/Provider/Share 与八种 auth-kind×region 单绑定、同账号恢复、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线伪名称不证明厂商实际发送；八份真实 receipt 仍独立 `live_pending`，采集不在本 goal 内。

验证：修复后的非流/流式固定反例通过；Kiro Rust library 229/229、API 合同筛选 3/3；八路私有 receipt 离线测试 492/492，冻结 reference-delta `--check-sources` 通过。最终工作树完整 Rust library 3623 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。`cargo fmt --check` 首次仅发现新测试排版差异，修正后通过；严格 Clippy 首轮仅因测试使用 `err().expect()` 失败，改为等价 `expect_err()` 后通过，定向及完整 Rust 也在该最终写法上重跑通过。`cargo check -q`、Provider coverage/UI 静态矩阵、文档索引及差分空白检查通过；生成 coverage 状态未变。`PROTOCOL_EVIDENCE.md` 与 Kiro Provider 文档已补脱敏边界。本轮未重跑全量后端 Node；§18.322 的 1909/1909 不是本轮结果。八类最终整体 review 尚未完成；没有 UI 自动化、部署测试、真实账号请求、提交或推送。

### 18.328 整体 review 的 Qoder 零主计数用量别名回退

2026-10-14，继续仅复核 Qoder。`Qoder/AGENTS.md` 仍首选 TokenRouter，其当前工作树仅删除根 `AGENTS.md`，仓内没有现存的同名指令文件；旧 `project-doc` 门禁不再限制当前只读研究，HEAD 仍追踪该文件，恢复时须重核。已提交 `TokenRouter@c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的 `backend/internal/upstream/qoder/client.go::ParseSSELine/qoderSSEMessageDeltaEvents` 对 Chat choice 和 event-envelope usage 都在 `prompt_tokens`/`completion_tokens` 为零时回退至 `input_tokens`/`output_tokens`，非零主计数优先。目标 `qoder_token_value` 原验证所有出现的字段，却总选第一个值；`prompt_tokens:0,input_tokens:5` 因此被规范化为零，可能低记用量。两种入口的 decoder→非流聚合固定反例先红，旧结果为 0 而预期为 5。

现只在主计数为零且后续别名非零时选别名；所有出现字段仍逐一通过现有限型验证，非法备用值不会被有效主值遮蔽。主计数非零、全零、数字字符串/有限小数的既有兼容、显式 `total_tokens` 一致性和溢出门槛保持。流式转换复用同一 canonical usage 入口；COSY 出站 endpoint/header/body/签名、Global OAuth/Global PAT/CN OAuth 三 rail、Account/Provider/Share 单绑定、唯一终态+EOF、同账号恢复与 receipt schema v2/revision 2 均未改。参考宽松的“总数为零则推导”不替代目标既有显式总数一致性规则。本地伪 usage 不证明厂商真实发此组合；三 rail 保持 `live_pending`，真实账号验收不在本 goal 内。

验证：反例旧实现先红，修后 Chat choice 与 `message_delta` 均通过；还固定了非零主字段优先、全零保持及非法备用字段拒绝。Qoder Rust library 128/128；完整 `CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 为 library 3624 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1。Qoder 私有 receipt 离线测试 281/281、Qoder reference-delta `--check-sources`、官方 CLI oracle、Provider coverage、UI Provider **静态矩阵**、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -- -D warnings` 及差分空白检查通过；生成 coverage 状态未变。`PROTOCOL_EVIDENCE.md` 与 Qoder Provider 文档已补本地计量证据。本轮未重跑后端 Node 全量 1909 项；此前 §18.322 的结果不是本轮结果。Qoder 本次最终 review 切片已收口，但八类合并后的整体 review 尚需下一类别 CodeBuddy 等余项；没有 UI 自动化、部署测试、真实账号请求、提交或推送。

### 18.329 整体 review 的 CodeBuddy 业务拒绝码与认证恢复边界

2026-10-14，继续仅复核 CodeBuddy。`CodeBuddy/AGENTS.md` 首选的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/workbuddy/client.go::Classify` 将 HTTP 401 与 `12153` 归为认证类，但没有定义本 Server 是否可刷新重放；两个未跟踪的 `proxy.html`、`proxy.md` 仍排除。目标 Provider 文档 §8.1 的 APISIX HTML 401 应保留一次同账号恢复；§8.2 的 `12005` 席位超限、`11212` 授权过期、`11216` 试用过期是不可恢复许可终态，§9.1 的 `12153` 需要重新登录。原 `CodeBuddyUpstreamError::is_authentication_failure` 只看 HTTP 401；HTTP 401 附带已知 `14003` 会错误刷新，首组 HTTP 回环在旧实现实测两次生成请求。初版修复虽挡住限流，却把映射为下游 401 的三个许可码及 `12153` 仍当作可恢复；旧 `LateUnauthorized` SSE 夹具也用 `12005` 虚构了可成功的刷新重放。该中间状态未作为结论交付。

现恢复资格要求原始 HTTP 401 且没有已识别的业务拒绝码：无业务码 HTML 401 和未知数字码 HTTP 401 在恢复资格判断上仍按原 HTTP 状态回退；`14003` 等已知限流/权限/请求拒绝、三项许可终态以及 `12153` 均不刷新，HTTP 403 也不因业务码映射为 401 而升级为可重试。`LateUnauthorized` 回环改为 `LateLicenseRejected`：非流在已收到正文但尚未提交下游时仍直接以 401 终止，不刷新绑定账号、不向诱饵账号发网；流式提交后无成功终态，亦不刷新。Intl/CN × Claude/Codex/Gemini × 流/非流的 48 组 HTTP 401 终态码回环各只发一次生成请求，刷新为零；另保留 12 组 HTTP 401+`14003` 限流回环、无业务码 HTTP 401 同账号一次恢复和单测中的 HTML/未知码回退。只改本地恢复决策和测试，不改 WorkBuddy 出站 endpoint/header/body、双站/三 Surface、Account/Provider/Share 单绑定、一次恢复预算、唯一 `[DONE]`+EOF、下游错误脱敏或 receipt schema v2/revision 2。离线合成错误不证明两站实际发送这些组合；Intl/CN 继续独立 `live_pending`，真实账号验收不在本 goal 内。

验证：最终 CodeBuddy 定向 Rust 113/113；新增 48 组终态码回环单独复跑 1/1；完整 Rust library 3627 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1。CodeBuddy 私有 receipt 离线夹具 186/186、冻结 reference-delta `--check-sources`、Provider conformance/coverage、UI Provider **静态矩阵**、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -- -D warnings` 与差分空白检查通过。本轮未重跑全量后端 Node 1909 项，§18.322 的结果不是本轮结果。`PROTOCOL_EVIDENCE.md` 与 CodeBuddy Provider 文档已补本地恢复边界，生成 coverage 状态未改。CodeBuddy 本次整体 review 切片已收口，但八类合并后的最终 review 仍未完成；没有 UI 自动化、部署测试、真实账号请求、提交或推送。

### 18.330 整体 review 的 Antigravity 纯思考尾部签名别名

2026-10-14，继续仅复核 Antigravity。`Antigravity/AGENTS.md` 推荐的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 和 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 均保持干净提交态。前者已提交 `a2718314` 在 Gemini 目标清理末尾纯思考 model/assistant turn，并在 `antigravityNativePartThoughtSignature` 识别 `thoughtSignature`、`thought_signature`、`extra_content.google.thought_signature`；后者仅作 pipeline/thinking store 交叉背景。目标 `native_signature` 同样识别这些别名，但 `drop_trailing_thought_only_model_turns` 仅白名单 `thoughtSignature`。最终 v1internal Gemini envelope 的固定 `thought_signature` 反例在旧实现先红：纯 thought 尾部仍留在 `request.contents`，可能碰到参考记录的“请求以 model turn 结束”拒绝。此为本地风险推断，不是 Google 真实错误复现。

现只把直接字符串 `thought_signature`、以及仅含 `google.thought_signature` 字符串的精确 `extra_content` 对象纳入已知 thought 元数据；嵌套还有其他字段时保留整 turn，正文、函数调用、媒体和其他未知字段仍不删。Gemini 尾部清理仍至少保留一条历史，Claude 最终模型及无 envelope 的 raw body 不变；首次封装和 replay 后归一化共用该边界。出站 endpoint/header、普通 body 其他字段、AG-N6 requestType、AG-D3 签名 scope/CAS、Account/Provider/Share 单绑定、同账号恢复、compaction disabled 和 receipt schema v2/revision 2 均未改。两条真实 OAuth rail、AG-N6 与 AG-D3 的厂商接受性保持 `live_pending`，真实账号验收不在本 goal 内。

验证：反例旧实现先红、修后通过；Antigravity Rust 定向 118/118，完整 Rust library 3628 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1（`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216`）。Antigravity 私有 receipt 离线夹具 165/165、冻结 reference-delta `--check-sources`、Provider coverage/UI **静态矩阵**、文档索引、`cargo fmt --check`、`cargo check -q`、严格 Clippy `--all-targets -- -D warnings` 与差分空白检查通过。本轮未重跑全量后端 Node 1909 项，§18.322 的结果不是本轮结果；没有真实账号、UI 自动化、部署测试、提交或推送。`PROTOCOL_EVIDENCE.md` 已补该边界，生成 coverage 状态未变。Antigravity 本次 review 切片已收口；八类合并后的跨 Provider 最终 review 仍未完成。

### 18.331 八类跨 Provider 离线整体 review 与交付边界

2026-10-14，在 §18.322～18.330 逐类 review 后，对照 §17.15、§18.2～18.4 的**当前可离线完成项**进行合并审计。第 0～17 节及 §18.1～18.4 中按旧日期冻结的“Qoder 仍受 `project-doc` 阻断”“真实 receipt 缺失则本 goal 不得结束”不是当前工作树判定：TokenRouter 当前只删除未提交的根 `AGENTS.md`，Qoder 可达 HEAD 已按 §18.302～18.320 补审；真实账号验收已由用户明确排除在本 goal 外，不能据此提升任何 `live_pending`。本节不改生产代码或外部仓。

| 计划要求 | 当前工作树的直接证据与判断 |
| --- | --- |
| 八类推荐参考与最新可达差分 | 当前 HEAD 分别为 CLIProxyAPI `bf934162`、Antigravity-Manager `b601f5e7`、codex2api `84a969d8`、OmniRoute `443d6699`、grok2api `7c889a96`、sub2api `ab99d56e`、kiro.rs `9066e9c4`、TokenRouter `c25dbf0c`、cli2api `6cf7e4eb`，与逐类 review 使用的提交一致。八个 reference-delta `--check-sources` 全通过；OmniRoute、sub2api、cli2api 的未提交业务外文件继续排除，TokenRouter 的 ` D AGENTS.md` 仅代表当前工作树，HEAD 中历史指令不改写。Antigravity、Claude、Codex、Cursor、Grok、Kiro、Qoder、CodeBuddy 的本轮 review 切片分别见 §18.330、§18.322～18.323、§18.324、§18.325、§18.326、§18.327、§18.328、§18.329。冻结对象通过不自动证明目标当前工作树已提交。 |
| R3-1 终态/工具与 R3-2 目录/计量 | CL-N8 的 `pause_turn` JSON/SSE 不冒充完成、CX-N13 精确空参数归一、KI-N4 零字节无参恢复均在现行源码和 Rust 测试中；AG-N12 的旧 token 晚到、成功空目录、代际证据 CAS 有状态/API 回环。KI-N5 仅保留 context-output 候选 shadow，明确 metrics/tokenUsage 仍优先，不在缺真实 receipt 时改收费权威。§18.5 后续各项反例和逐类复核记录仍为局部依据，本节全量绿灯不取代红灯记录。 |
| R3-3 差分后决策与不采纳项 | AG-D3 的双层 CAQS、多图历史 hash、并行工具和终态后 replay，CL-N7 的作用域化续线程，CX-D3/D4 的官方客户端精确 schema/终态差分，以及 CB-D3 的双站取消与最终 chunk 账务，均已有对应工作树实现、回环或明确绿/红记录。账号池/跨账号 fallback、动态未验证客户端指纹、伪造签名、静默丢坏工具未迁入；GR-N4 不凭版本参考改 wire，Kiro Compact/共享 cache、Grok remote compaction、Antigravity Compact 等无真实证据能力继续 disabled。 |
| 共享身份、资源与提交边界 | `ForwardAttemptContext` 统一保留 request 内 attempt/elapsed/memory 预算、`CommitGuard` 和 `BindingSnapshot`；主转发循环每次请求前重核 Provider/Account/Share 绑定，刷新只允许同账号代际前进，提交后不透明重放。各 Provider 的 §18.322～18.330 回归验证自身 terminal/EOF、取消、用量和 decoy；本节重新运行完整 Rust 与后端 Node，而不是用单一类别定向测试支持跨 Provider 结论。Provider conformance、Phase 0 writer、SQLite authority、source provenance、产品/网络边界、transform coverage、生成 coverage、UI Provider **静态矩阵**与文档索引审计均通过。 |
| 真实与提交态证据边界 | 所有真实 rail/operation 仍 `live_pending`，真实账号请求不在本 goal 内。当前目标 `HEAD=65451910144b8460ed5fa36d2c9c926936022de6`，有大量未提交实现和一个未跟踪但已由 Rust 编译/测试实际引用的 `src/proxy/claude_thread_alias.rs`；故只能称**当前工作树离线整体 review**，不能补写新的 `committed-target` observation、宣称已提交/推送，或用冻结旧对象替代当前源码 provenance。若交付要求提交态 observation，须先在明确的提交工作流中冻结并复核目标 tree；本轮不擅自提交整棵已有脏工作树。 |

当前完整离线验证：`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 为 library 3628 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1；独立后端 Node audit/smoke 1909/1909。八类参考 `--check-sources`、Qoder 官方 CLI oracle、Provider conformance/contract/Phase 0/coverage、SQLite、coding-plan/web-session registry、产品/Web/source 边界、transform coverage、文档索引、JSON 与脚本语法、`cargo fmt --check`、`cargo check -q`、严格 Clippy 和差分空白检查通过。本地服务 smoke 首次因 30 秒启动窗口内未监听而无服务行为结论；在同一 `CARGO_INCREMENTAL=0` 编译环境预构建后，独立临时配置和 127.0.0.1 端口复跑通过 health/version、Web fallback、offline setup、密码/API token 登录、Provider/Share 创建。此前 §18.321 的首轮 Node 1905/1909 波动仍保留，当前独立完整回归通过不抹去历史。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行；没有做 UI 自动化、部署测试、真实账号验收、提交或推送。

**判断**：八类当前工作树的离线实施与合并 review 已有相称证据，未发现本节新增的可离线生产缺口；但计划要求的当前实现 `committed-target` 冻结证据仍缺，不能把本 goal 宣告为已完成的提交态交付。真实 receipt 的缺失不属于本 goal 的剩余项，保持独立后续验收。下一步只应在明确提交范围后复核 staged/committed tree、补对应不可变 observation 与最后一次离线检查；若交付仅要求工作树实现，应明确区分该边界，不把本节的工作树审计写成提交态结论。

### 18.332 当前工作树交付前复验（2026-10-14）

在 §18.331 之后重新核对现有工作树：目标 `HEAD` 与 `origin/main` 均为 `65451910144b8460ed5fa36d2c9c926936022de6`，仍有 93 个已修改跟踪路径和一个已被 `src/proxy/mod.rs`、`src/state.rs`、转发路径引用的未跟踪实现文件 `src/proxy/claude_thread_alias.rs`。变更路径均位于本次增强的代码、合同、审计、夹具或文档范围；路径归属检查不替代逐项 staged diff 复核。`Cargo.toml` 的 GIF feature 与 Kiro/Cursor 图片处理路径对应，`Cargo.lock` 同步依赖。没有暂存、提交或推送这些工作树改动。

当前独立复验：`CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 为 library 3628 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1；`node --test scripts/audit/*.test.mjs` 为 1896/1896，`node --test scripts/smoke/*.test.mjs` 为 13/13；`cargo fmt --check`、严格 Clippy `--all-targets -- -D warnings`、Provider coverage 生成检查、`git diff --check` 和八类 reference-delta `--check-sources` 均通过。TokenRouter 当前工作树仅删除未提交的根 `AGENTS.md`，其路径上无其他适用的同名文件；Qoder 参考源码审计可运行且通过，但该文件若恢复须重新核对门禁。完整 `scripts/static-checks.sh` 含 Web UI 测试，仍未运行；本轮没有 UI 自动化、部署测试或真实账号请求，各真实 rail/operation 保持 `live_pending`。

本节只加强**当前工作树**的离线验证，不产生新的 `committed-target` observation；提交态交付仍需先确定精确提交范围，再对 staged/committed tree 复核。真实账号验收已明确在本 goal 外，不得作为提交态证据的替代品，也不因本节测试通过而提升状态。

### 18.333 Claude 续线程内联工具定义的历史别名门槛（2026-10-15）

本次只处理 Claude。整体 review 对照目标自身 `claude_oauth::rewrite_claude_tool_change_name` 与 `claude_thread_alias::needs_prior_alias_state`：前者能在 `tool_addition.tool_definition.definition.name` 的非 server 内联定义中恢复旧别名，后者原只检查工具增删中的 `tool_reference`。只声明新工具、历史内联定义使用旧非法工具名的固定单测旧实现先报 `needs_state=false`；实际 HTTP 回环也先发上游请求并得到 502，而预期是缺旧状态时发网前 404。该差异来自目标现有协议位置不一致，不推断新的 Anthropic wire。

现将预检与改写共用工具增删名称位置判定：未由本轮声明且确实需要别名的旧内联定义须取得同一精确线程状态；有状态时复用旧 wire 名，缺状态时 404、零上游请求。`web_search_*` 等 server 定义不被误拦，普通文本、工具 `input`、无需改名的合法名称和 `tool_removal` 非引用形状不扩大。Claude alias 定向 16/16 通过；本次改动后的完整 Rust library 3628 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1，Claude 私有 receipt 离线测试 252/252；Claude reference-delta `--check-sources`、Provider coverage、文档索引、`cargo fmt --check`、严格 Clippy `--all-targets -- -D warnings` 和差分空白检查通过。随后在同一生产代码工作树上重跑后端 Node 全量，结果见 §18.334。缓存身份、TTL/容量、OAuth endpoint/header、Account/Provider/Share 单绑定、成功终态、usage 与 receipt schema v1/revision 1 均不改。真实账号验收继续在本 goal 外，五项 operation 及续线程厂商接受性仍 `live_pending`；当前工作树未提交，不写新增 `committed-target` observation。没有运行 UI 自动化或部署测试。

### 18.334 Claude 修复后的八类离线整体复核（2026-10-15）

§18.333 只改变 Claude 续线程历史工具定义的本地发网前门槛及对应测试/文档，不改变其余七类 Provider、共享身份/预算/终态合同或任何真实状态。本节对照 §18.331 的八类合并审计和 §18.2～18.4 的当前可离线项复验：完整 `CARGO_INCREMENTAL=0 RUST_MIN_STACK=16777216 cargo test -q` 为 Rust library 3628 passed / 1 ignored、API 合同 124/124、其余两项集成各 1/1；`node --test scripts/audit/*.test.mjs` 为 1896/1896，`node --test scripts/smoke/*.test.mjs` 为 13/13。八类 reference-delta `--check-sources` 均通过；Qoder CLI oracle、Provider conformance/contract/Phase 0/coverage、SQLite authority、coding-plan/web-session registry、产品/Web/source 边界、transform coverage、UI Provider **静态矩阵**与文档索引均通过。Claude 定向 alias 16/16、私有 receipt 离线夹具 252/252、`cargo fmt --check`、严格 Clippy `--all-targets -- -D warnings` 和差分空白检查也通过。

本轮没有发现 Claude 修复引起的新跨 Provider 离线缺口，但这仍是**当前工作树**结论：目标 `HEAD` 与 `origin/main` 仍为 `65451910144b8460ed5fa36d2c9c926936022de6`，94 个任务相关路径（93 个已修改、1 个未跟踪实现）尚未形成新的 implementation commit/tree，旧 reference-delta observation 不能证明本次工作树已提交。真实账号验收不在本 goal 内，各 rail/operation 保持 `live_pending`；没有做真实请求、UI 自动化或部署测试。完整 `scripts/static-checks.sh` 包含 Web UI 测试，按禁令未运行。本 goal 若要求提交态交付，剩余步骤仍是确定提交范围、复核 staged/committed tree 并追加真实的不可变 target observation；不得把本节的离线绿灯写成提交态或 live 证据。

### 18.335 八类实现提交态冻结（2026-10-15）

在用户明确要求 `commit push` 后，精确复核并暂存 §18.334 的 94 个任务路径，`git diff --cached --check` 通过；实现提交为 `b9ef3c9abf8123ea36d0438bd027952e56ad9284`，父提交 `65451910144b8460ed5fa36d2c9c926936022de6`，tree `0ed8f7e6bdbf6da1e2076800b0673a4ed6f52da1`。此前八类逐项与整体复核的工作树现在对应一个真实、可读取的 Git 对象；这不追溯改变 §18.331～18.334 当时“未提交”的事实。

新增独立的 append-only 提交态 revision `assets/contract/provider-enhancement-committed-target.json`，逐类固定该实现提交、各自 reference-delta 的已提交 SHA-256、实现路径与离线 fixture 锚点；`scripts/audit/audit-provider-enhancement-committed-target.mjs` 校验 revision 摘要、提交父子/tree、八类清单、Git 对象内容及实现/fixture 确实位于父子差分，并接入常规静态检查。八个原有 reference-delta 历史 observation 均保持原样；此索引只补当前实现的 `committed-target` 证据，不能提升任何真实 rail/operation。

在相同生产代码的提交态上复跑：Rust library 3628 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1；后端 Node audit 1896/1896、smoke 13/13。八类 reference-delta `--check-sources`、新增提交态审计、Qoder CLI oracle、Provider coverage、UI Provider **静态矩阵**、文档索引、`cargo fmt --check`、脚本语法和差分空白检查均通过。完整 `scripts/static-checks.sh` 含 Web UI 测试，仍未运行；没有真实账号请求、UI 自动化或部署测试。推送是否成功只以远端核对结果为准，不由本地提交态推断。

<a id="enhance-r4-20261027"></a>

## 19. 八类推荐反代的源码复核与 R4 增强计划（2026-10-27）

本轮交付是**对比分析和增强计划**。以下改造、回归用例和真实验收都是后续工作，不能与第 18 节已经实施的 N 系列混用。本轮未修改生产代码、协议合同、能力开关或真实验收状态。

### 19.1 研究范围、冻结点与判断方法

目标为 `/data/projects/cc-switch-server`，冻结 `HEAD=e5b9e83ad488448473ad604fb855fbe28dd5f456`、tree `6fe459e01731bdb78c27548efef08660c8a84606`；研究开始时工作树干净。已核对 [开发约定](AGENTS.md)、[架构总览](docs/architecture/overview.md)、[协议证据](PROTOCOL_EVIDENCE.md) 和 [生成的 Provider coverage](docs/provider/coverage.md)。当前代码晚于 §18.335，因此以当前源码、调用链和已有测试确认“已实现”，不从旧计划的未勾选项推断缺失。

已逐个读取 `/data/projects/proxy/{Antigravity,Claude,Codex,Cursor,Grok,Kiro,Qoder,CodeBuddy}/AGENTS.md`，沿推荐路径研究以下九个不同仓库。表中的路径相对 `/data/projects/proxy`；类别目录中的软链接不重复计为独立证据。冻结的是**本地可达提交**，本轮没有拉取远端，不能称为互联网最新版本。

| 类别 | 推荐仓库及实际路径 | 本轮冻结 HEAD | 工作树处理 |
| --- | --- | --- | --- |
| Antigravity | `CLIProxyAPI` | `1f917e77c4562623aa49e9804d999524151a2f96` | 干净；同时供 Claude/Codex 对照 |
| Antigravity | `Antigravity/Antigravity-Manager` | `b601f5e7375343343922ccab5e50653bc3450c3b` | 干净 |
| Claude | `CLIProxyAPI` | `1f917e77c4562623aa49e9804d999524151a2f96` | 同上，专查 Claude executor/translator |
| Codex | `CLIProxyAPI`、`Codex/codex2api` | 前者同上；后者 `3c6adfda702efed67931b2d98a491d72bd9d91b5` | codex2api 干净 |
| Cursor | `OmniRoute` | `443d66996d69d7ecf887fd61158404a3cca1b192` | 21 个 tracked 改动、1 个 untracked；业务源码仅读提交对象 |
| Grok | `Grok/grok2api` | `e2fd7ab481a30a1965bb61395d437154cf247c92` | 干净 |
| Grok | `sub2api` | `ab99d56e9626e6cd731592dae8553c9758a0efa2` | 6 个 tracked 改动；业务源码仅读提交对象 |
| Kiro | `Kiro/kiro.rs` | `9066e9c4fe90c7e97d241900a6ce8eb52cc46903` | 干净 |
| Qoder | `TokenRouter` | `d15137eb59d4d85bdc27ae563c37fcf9b0edebcd` | 根 `AGENTS.md` 为 `UD`；不解决冲突，业务源码仅读提交对象 |
| CodeBuddy | `cli2api` | `c7e4e73f64e661028a235e71306dba54ece48435` | 排除 untracked `proxy.html`、`proxy.md` |

TokenRouter 当前根指令可读，已按其要求读取 `.agents/skills/project-doc/SKILL.md`、`.agents/skills/code-comments/SKILL.md`、`docs/index.md` 以及 Qoder 接口/架构入口，核对 `qoder_execution_boundary` 和 `qoder_account_contract`。历史“根指令删除，技能不适用”的描述只属于当时快照，本轮没有沿用。所有外仓均只读，未复制账号、私有配置或未提交业务变更。

相对 §18.335，CLIProxyAPI、codex2api、grok2api、TokenRouter、cli2api 出现新提交；Antigravity-Manager、OmniRoute、sub2api、kiro.rs 的冻结 HEAD 未变。后四个仍与目标当前实现重新对照；“参考未变”不代表目标无需复核。

本轮沿“凭据/身份 → 请求与工具转换 → 上游传输 → 流式状态与终态 → 用量/目录 → 取消、恢复和缓存”检查。结论采用以下状态，避免把参考项目的策略直接升格为协议规范：

| 状态 | 含义 | 后续动作 |
| --- | --- | --- |
| `confirmed_gap` | 源码可以直接证明丢信息、身份不完整、错误分类或资源保护缺口 | 先在本仓固化失败用例，再做窄范围修复；不等于厂商线上故障已复现 |
| `differential_first` | 双方行为有差异，但新行为的接受性或本产品适用性尚不充分 | 先比较最终出站报文和状态转换，形成取舍记录，不能直接替换 |
| `live_pending` | 离线材料不足以证明真实账号、站点、模型接受 | 逐 rail/operation 留存私有验收回执；不自动启用能力 |
| `reviewed_no_new_gap` | 本次检查范围未发现新的可确认专用协议缺口 | 保留基线和验证任务，不虚构改造项目 |
| `not_adopted` | 与固定绑定、语义保真、产品范围不符，或证据不足且风险高 | 明确拒绝迁入，除非后来有新的需求与证据 |

### 19.2 当前结论与优先顺序

目标已经具备统一固定绑定、同账号刷新代际、恢复预算、流式终态、请求生命周期内存预算和多种 Provider 专用续接能力。主要薄弱点集中在**跨协议信息保真、动态身份与证据的一致性、控制帧分类、进程级容量**。无需重建 Provider 框架、账号池或数据库层。

| 编号 | 优先级 / 状态 | 可定位的薄弱点 | 主要影响 |
| --- | --- | --- | --- |
| R4-CORE-01 | P0 / `confirmed_gap` | namespace 工具压平后碰撞，声明静默去重，历史名称与活动工具共用回退规则 | 错误工具身份、丢声明或错误回映；涉及使用共享 bridge 的多类 Provider |
| R4-AG-01 | P1 / `confirmed_gap` | 模型目录缓存和发布 CAS 没有包含动态 Antigravity UA/版本 | 版本变化后复用旧能力，或旧请求晚到污染新身份观察 |
| R4-CL-01 | P1 / `confirmed_gap` | Claude `thinking_tokens` 原样复制进 OpenAI details，未归一为 `reasoning_tokens` | 下游推理用量缺失或使用错误字段 |
| R4-CX-01 | P1 / `confirmed_gap` | Responses message 的 `commentary` / `final_answer` 阶段在转换时丢失 | 过程文本混入最终文本、跨协议历史无法保真；映射方案先差分 |
| R4-CX-02 | P1 / `confirmed_gap` | WS 配额仅记录部分 reset window，frame 方言与 metadata 用量覆盖不足 | 账户容量快照滞后；不能等同于请求 token 统计缺失 |
| R4-CX-03 | P1 / `confirmed_gap` | 原生 WS 的已知 metadata 被通用分类器当作业务输出 | 首有效内容计时、提交标记和空闲期限被控制信息推进 |
| R4-CX-04 | P1 / `confirmed_gap` | interrupt 的 mode、ACK 和 HTTP 回退局部语义不完整 | 控制失败可能被当生成失败；回退对丢弃部分 item 的语义不明确 |
| R4-CORE-02 | P1 / `confirmed_gap` | 单请求预算之外缺统一进程保留字节预算 | 多请求及会话缓存同时达峰时仍可耗尽进程内存 |
| R4-AG-02、R4-CL-02、R4-GR-01、R4-QD-01、R4-CB-01 | P1 / `differential_first` | 签名/模型约束/重试头/历史保留/客户端身份的新差异 | 差分成功才进入生产修改，不预定照搬结论 |
| R4-CUR-01、R4-KI-01、R4-LIVE-01 | P1 / 验证与 `live_pending` | 专用协议已较完整，主要缺当前真实回执和计量/资源证据 | 按账号类型、区域、站点、operation 独立验收 |
| R4-CX-05、R4-CORE-03、R4-QD-02 | P2 / 测量或补证后决策 | 出站压缩、统一性能里程碑、热点代码拆分及 CN PAT rail 候选 | 性能项先测量，新增 rail 先验证独立凭据合同 |

这里的 P0 表示优先阻断可证明的工具身份混淆，不表示已经发现外部攻击或线上事故。上述静态反例尚未作为新的 Rust 测试运行；本轮的检查结果见 §19.15。

### 19.3 共享协议与资源层

#### R4-CORE-01：工具声明、历史与回映使用独立且无碰撞的身份

**目标证据。** [transforms.rs](src/proxy/transforms.rs) 中 `ResponsesToolContext::add_spec`（约 222 行）在 `chat_name` 重复时直接返回；`flatten_namespace_tool_name`（约 838 行）拼接 `namespace__name`，长名 hash 的输入仍是拼接后的字符串；`openai_response_tools_to_chat`（约 4171 行）按压平名称只保留首个工具。`validate_response_tool_type_collisions`（约 4115 行）只覆盖普通 function 与 custom 同名，没有覆盖 namespace 产生的碰撞。`chat_name_for_response_function`（约 140 行）对未声明历史仍直接压平。

可由上述函数直接推出两个反例：`(namespace=a, name=b__c)` 与 `(namespace=a__b, name=c)` 都得到 `a__b__c`；普通 function `browser__js` 与 namespace `browser` 下的 `js` 也得到同名。顺序变化会改变被保留的声明及响应回映所有者。长名加 hash 不能修复相同拼接串；现有正常 namespace roundtrip 测试不足以覆盖此问题。

**参考证据。** TokenRouter 的 `backend/internal/protocol/bridge/responses_tool_identity.go` 用规范身份和 owner 表拒绝冲突；`responses_tool_history.go` 单独分配历史名称，预留活动 function/custom/tool_search 名称，按原始 `(namespace,name)` 排序，分组件编码后 hash，且不把历史身份授权给 selector 或未来工具调用。关键提交为 `b7967bdbb`、`f4a639dd4`、`99cbf5049`，配套 `_test.go` 包含声明碰撞和未声明历史用例。

**实施计划。** 在本仓共享 bridge 定义 `(kind, namespace, original_name)` 身份及请求内不可变双向映射。第一切片对活动声明的歧义在发网前稳定拒绝，消除静默首个胜出；第二切片为合法但未再声明的历史建立独立名称表，排序分配、长度受限、保留名冲突可检测。工具声明、`tool_choice`、历史调用和响应回映必须共用已审核的身份来源，历史表只服务历史。原生无需转换的 Responses 路径保持其原生名称；不要在 Qoder、CodeBuddy executor 各复制一份算法。

**验收。** 覆盖两个具体反例及工具顺序置换、长 UTF-8 名、custom 与内部保留名、仅 `tool_search`/`tools=[]` 的历史、`additional_tools`/发现结果、未声明 selector、同名不同 kind、流式与非流式回映。参数、结果和普通文本中的同名字串不能被递归替换。跨 Provider 调用链应验证“拒绝时零上游请求”和原有无冲突工具往返。Qoder 后续还会筛选历史，须与 R4-QD-01 联合验证，不能仅凭 bridge 单测宣称端到端保留成功。

**风险与回退。** 旧客户端可能依赖首个胜出；应返回明确冲突错误，而不是维持歧义执行。历史别名兼容需要遵守现有身份作用域和 TTL；回退可以撤销新增历史支持，但不能重新开放已知身份碰撞。

#### R4-CORE-02：在单请求预算之上增加进程总预算

**目标证据。** [request_memory.rs](src/proxy/request_memory.rs) 的 `RequestMemoryBudget::new`（约 189 行）为每次请求创建独立 `Arc/Mutex` 账本。`requestBodyLimits.memoryBudgetMb` 是单请求生命周期预算，默认 256 MiB；已有 Share/Account 并发准入和各缓存上限，但没有一个把这些保留字节合并约束的进程父预算。目标并非“没有内存保护”：八类 sticky budget、压缩解码、图像工作集、重试和 retained state 已有大量防护。

**参考证据。** codex2api 的 `security/request_memory.go` 用进程 singleton `processRequestMemory`、`TryAcquireRequestMemory`、resize/release 和高水位快照约束并发保留量；注释明确它统计逻辑保留字节，不能冒充 RSS。该原则比它的默认数值更值得吸收。

**实施计划。** 在现有 reservation 上增加进程父预算，保持单请求 sticky exhausted 语义；子预算与父预算增长须整体成功或撤销，缩容和 RAII 释放始终可完成。先覆盖 HTTP/WS 活跃请求，再处理图像、parked session、previous-response/replay 等跨请求保留对象的所有权转移；共享同一 buffer 只能按真实所有者计一次。进程额度不足归类为本地 capacity shed，不刷新凭据、不扩大 Provider cooldown、不跨账号重试。

**验收。** N 个各自未超限的请求合计超限时可控拒绝；覆盖创建失败、读取增长、gzip/图片峰值、取消、双重 drop、会话停放/恢复/淘汰、重试复用、动态降低上限与并发 resize。暴露 aggregate used/high-water/rejections/active reservations，压力测试同时观测 RSS，解释两者差值。预算默认值根据部署内存和测量确定，不抄参考仓的 128 MiB。

**风险与回退。** 重点防止双计、泄漏和持锁发网；进程预算先可观测，再启用硬准入。回退父预算不撤掉既有单请求限制，已占用内存不通过重置计数“释放”。

### 19.4 Antigravity：目录身份 fencing 与签名差分

| 方面 | 推荐实现 | 目标当前实现与判断 |
| --- | --- | --- |
| 目录能力与身份 | CLIProxyAPI `sdk/cliproxy/antigravity_models.go` 捕获 UA，并纳入 capability/backoff key；更新时控制旧 probe 的发布 | 目标已有 token/账号代际、成功空目录和晚到响应 CAS；缺 UA 版本维度，见 R4-AG-01 |
| 多轮签名 | CLIProxyAPI `internal/cache/antigravity_reasoning_replay_cache.go` 及 executor replay 测试持续扩展 | [antigravity_replay.rs](src/proxy/antigravity_replay.rs) 已有多维身份、TTL、工具 ID/历史摘要、多图与终态约束；新形状需差分 |
| thinking/usage 结构 | Antigravity-Manager `src-tauri/src/proxy/pipeline/{mod,policy,inbound,usage}.rs` 收敛分类和 canonical usage | 可借鉴小模块职责划分，目标已有自己的 Gemini/Anthropic/Responses 语义，不能整体替换 IR |
| 空思考“自愈” | Manager `pipeline/auto_heal.rs` 在已输出思考后追加“继续”并拼接下一次生成 | `not_adopted`：会引入额外请求、改变提示与终态，破坏本仓提交/用量边界 |

#### R4-AG-01（P1，`confirmed_gap`）

[provider_identity.rs](src/provider_identity.rs) 的 `spawn_antigravity_version_updater` 每三小时更新运行时版本；`antigravity_user_agent()` 根据当前值生成 UA。[antigravity_models.rs](src/clients/oauth/antigravity_models.rs) 的 `CacheKey`（约 86 行）包含账号 incarnation、auth/token generation、project、URL 和 token fingerprint，但不包含 UA。建 key（约 218 行）和发送请求（约 237 行）又分别读取身份。`state.rs::record_antigravity_model_catalog_observations_for_generation`（约 6476 行）及其消费者同样没有版本 fencing。于是同账号同 token 下，版本 A 的缓存/晚到目录仍可被版本 B 使用。

参考 CLIProxyAPI `02e9e852` 及 `antigravityCapabilityKeyForRoute`：在开始时捕获客户端身份，key 与实际发送使用同一 UA。目标应把**已捕获的版本/UA身份及其 generation** 贯穿 fetch、stale fallback、state 发布和能力消费；版本更改后旧观察不能作为新版本的 fresh 能力，旧进行中请求不能回填新代际。不能只清一次缓存而遗漏持久观察或正在执行的请求。

验收矩阵：同 token 切版本、读取版本与发送之间更新、旧请求晚到、版本更新失败、成功空目录、新版请求失败后的 stale 行为，以及两个 OAuth rail、不同项目/endpoint 的隔离。保持现有 token CAS；不将缺目录推断成全模型支持。回退必须同时处理版本字段与旧证据失效，不能只回退 HTTP UA。

#### R4-AG-02（P1，`differential_first`）

CLIProxyAPI `80aa2b1d`、`da21f1e5` 对 signed thought 前导、unsigned function call、非流式 replay、流式全局工具索引和 `tool_choice=none` 有新覆盖。目标 `capture_turn` 已能关联部分后置空签名 carrier，但不能由局部函数推出所有前置形状等价。

先构造完整两轮、最终 v1internal 报文差分：前置 signed thought→两个并行 call；call/text/call 分组；纯签名块；非流式→流式续接；caller 或账号代际变化；`tool_choice=none` 下历史声明和当前执行权限。只对能够复现的丢失补实现。Gemini sanitizer 的独立变化不能自动认定为 Antigravity 缺陷；compact 和未验证模型能力继续保持现有门禁。

### 19.5 Claude：usage 方言与模型约束

**已实现基线。** [claude_oauth.rs](src/proxy/claude_oauth.rs)、[claude_thread_alias.rs](src/proxy/claude_thread_alias.rs)、[anthropic_cache_control.rs](src/proxy/anthropic_cache_control.rs) 已覆盖 OAuth 身份、cache breakpoint/TTL、工具别名与线程状态、签名不透明性、429 作用域和固定绑定刷新。共享转换已把 `model_context_window_exceeded` 映射为 Chat `length` / Responses incomplete，refusal 有专用映射；这些不再列入待开发。

#### R4-CL-01（P1，`confirmed_gap`）

目标 `transforms.rs::openai_usage_from_anthropic_usage`（约 7125 行）和 `openai_responses_usage_from_anthropic_usage`（约 7152 行）将 `output_tokens_details` 直接 clone。输入 `output_tokens=10, output_tokens_details.thinking_tokens=4` 时，下游得到 `thinking_tokens`，规范读取 `reasoning_tokens` 的 Chat/Responses 客户端得不到对应明细。

CLIProxyAPI `43f5386b` 在 `internal/translator/claude/openai/chat-completions/claude_openai_response.go` 的 `claudeUsageTokens.Merge` / `applyReasoningTokens` 明确映射字段，区分缺失与显式零，且 reasoning 是 output 的子集。

计划在本仓共享 usage 转换层按目标方言归一；JSON 与流式累计结束使用同一规则，原生 Anthropic 保留其字段。验收包含 start/delta 分片、重复终态、details 缺失与零、非法类型/负值、同时出现两种名称时的冲突规则，以及 cache read/write 和 provider metering 保真。总 output 仍为 10，不能再加 4。回退可撤新字段映射，不能改变总 token 或重复写 usage。

#### R4-CL-02（P1，`differential_first`）

CLIProxyAPI `1d45a5f3` 的 `internal/runtime/executor/helps/claude_tool_choice.go` 按 routed Claude 5.5 模型把强制工具降为 auto，以保留 thinking；目标 `reconcile_forced_tool_choice` 对 any/tool 统一删除 thinking/context_management。双方有明确策略差异，但把 required 静默降 auto 同样违背调用者约束。

先按实际上游模型及入口（生成/count_tokens，直接 Claude/Antigravity 委托）比较最终请求和真实接受性，建立模型能力规则。若不能同时满足强制工具与 thinking，应明确拒绝该组合，或采用经过验证且保持强制结果的同绑定策略；不能擅自放宽工具选择或删除签名历史。保留无关模型行为。另将 `pause_turn` 放入策略差分：本仓已有 incomplete/null 及测试，参考的新 completed 表达不是直接替换理由，必须验证续线程、终态与客户端行为。

### 19.6 Codex：阶段保真、WS 观察与控制协议

**已实现基线。** [providers/codex/mod.rs](src/proxy/providers/codex/mod.rs)、[response_semantics.rs](src/proxy/response_semantics.rs)、[forwarder.rs](src/proxy/forwarder.rs) 已包含 OAuth/Responses、原生 WS、HTTP 回退、固定绑定和 token fencing、空启动事件缓冲、终态校验、工具参数补齐、previous-response 上下文、请求内存预算。当前源码还已支持 `parallel_tool_calls=false` 和带 sequence 的 keepalive；中断也已有原生透传和 HTTP 回退处理。不能把这些写成从零建设任务。

#### R4-CX-01（P1，`confirmed_gap`；呈现与回放方案先差分）

CLIProxyAPI `376015c3` 在 `internal/translator/codex/claude/codex_claude_response.go` 区分 `phase=commentary` 与 `final_answer`，并在 message item 边界重置 delta 状态；`531721bf` 又加入内部 marker 恢复历史，配有 commentary phase/replay/cross-format 测试。

目标 `transforms.rs::openai_responses_response_to_anthropic`（约 1569 行）和 `openai_responses_response_to_chat`（约 1631 行）只按 message/content 转文本；`stream_transforms.rs` 对 message 的 text/done 路径也不保留 phase。因此 `commentary: 正在检查文件` 和 `final_answer: 已完成修改` 会失去阶段区别。这个结论针对转换路径，不是说原生 Responses 透传必然丢 phase。

计划先在内部 message/item 状态保留 phase，明确无 phase 的兼容行为及目标协议的有损边界。原生 Responses 保留原字段；Messages/Chat 的展示与回放载体须单独形成合同，不能直接把公开 commentary 当成真实 reasoning，也不能照搬 `cpa:commentary:v1` 冒充供应商签名。需要借助服务器状态时，必须绑定现有 Share/用户/Provider/Account 身份、TTL 和内存预算；无法保真时应记录明确降级，不能静默制造 encrypted reasoning。

验收：多 message item、commentary→工具→final、done-only item 跟在已发 delta 的 item 后、JSON/SSE 一致、真实 encrypted_content 与 commentary 相邻、历史再入 Codex、历史转其他 Provider。不得漏最后一条回答、重复输出或把内部标记发给上游。交付分成“阶段捕获及失败用例”和“已审核的协议映射”两片，不为修 UI 展示而先引入私有签名协议。

#### R4-CX-02（P1，`confirmed_gap`）

目标 `providers/codex/mod.rs::record_quota_websocket_payload`（约 297 行）仅接受 `codex.rate_limits` / `rate_limits.updated`，然后交给 `domain/sharing/quota_cycle_source.rs::codex_observation` 记录周窗口。后者读取 `rate_limit.primary_window/secondary_window` 与 `limit_window_seconds`，不读取参考 frame 的 `rate_limits.primary/secondary`；也未从 `codex.response.metadata`/`response.metadata` 的 allowlist headers 更新 used_percent、plan、credits。目标已有其他账户用量查询和 HTTP header 处理，因此缺口是 **WS 数据来源接入不完整**。

codex2api `proxy/codex_usage_frames.go` 将上述 frames 规范化，隔离 metered/Spark 等非默认 bucket；credits 布尔值保留存在性。`auth/usage_frame_observation.go` 和 `auth/usage_frame_persist.go` 使用有界观察队列、身份/时间排序及合并持久化，相关提交包括 `376b9a0c`、`3fdbd449`、`1c759fa3`。

计划先冻结脱敏 frame 方言，新增小型 observer，把“上游账号剩余额度观察”与“本次推理 token 统计/Share 限额”分开；未知 bucket 不写主窗口，缺字段不覆盖较完整快照，重置时间禁止溢出。观察通过 `state.rs` 域方法发布，关联账号 incarnation/auth/token generation 和观察时间。当前 WS read loop 会 await quota 记录（`forwarder.rs` 约 13017 行）；加入更多写入时采用有界异步合并，避免把数据库延迟引入每帧读取，不直接搬参考的账号池或 cooldown 写法。

验收：两种 rate_limits 结构、metadata header 白名单、primary/secondary、缺失/零/false、额外 bucket、绝对/相对 reset、非法大数、旧 token 晚到、乱序/重复帧、队列满及持久化失败、重启恢复。关闭 observer 后推理仍可用，原有权威查询可恢复新鲜度；不得通过 metadata 推断一笔新的 token 消耗。

#### R4-CX-03（P1，`confirmed_gap`）

CLIProxyAPI `9e79552f` 将 `response.metadata` 纳入 `isCodexBootstrapBufferableEvent`；codex2api `proxy/overflow_compact.go::isCodexPreflightSSEEvent` 和 `proxy/wsrelay/read_pump.go` 分开处理前置信息、业务帧和空闲期遥测。

目标 `response_semantics.rs::classify_value`（约 420 行）对未匹配事件默认返回 Business。原生 WS 的 `classify_responses_websocket_message`（约 17011 行）直接调用它，所以无内容的 `response.metadata`、`codex.response.metadata`、`codex.rate_limits` 会计为业务输出，推进 `first_token_ms`、`emitted_business_event` 和 idle deadline。**不能把该结论直接套到所有 SSE：** public SSE 有 closed event visibility，裸 `response.metadata` 会先被过滤；native Codex 可见集合又有自己的例外，应分别测试。

计划引入精确的 metadata/control 分类，分开“允许转交”“用于配额观察”“首有效内容”“生成终态”“transport 已提交”。只对已审核的类型及形状生效，未知 `response.*` 不能笼统当作可丢遥测。验收覆盖 metadata→overload、metadata→EOF、metadata→文本、空闲期迟到 telemetry、错误帧优先级以及事件名/payload.type 冲突。

目标 `responses_websocket_http_replay_allowed`（约 11027 行）在 Codex `response.create` 已发出后禁止透明 HTTP 重放，这个边界必须保留。修 metadata 分类的收益是语义与计时正确，不能借机采用参考仓更宽的重放窗口。

#### R4-CX-04（P1，`confirmed_gap`；原生和回退分别收口）

codex2api `6794275a` 的 `proxy/responses_ws_interrupt.go` 校验 `mode=discard_partial_items`、response/stream 身份，跟踪 `response.interrupt.accepted/failed`，处理重复/已结束目标和 ACK 缺失；控制帧不会触发重连或请求重放。

目标 `codex_websocket_interrupt_id`（约 16943 行）只验证非空 response_id，不验证 mode。HTTP 回退匹配 ID 后直接合成 `response.incomplete`，output 来自 `collected_output_items()`（约 16167 行），没有独立 accepted/failed ACK。原生 WS 会转交上游帧，不能声称所有 ACK 均被丢弃；但通用 `classify_value` 对 accepted 默认判 Business，对带顶层 error 的 interrupt.failed 则会进入生成 Failure 分类，缺少控制失败与生成失败的区分。

计划建立最小 interrupt 状态：精确活动 response/连接身份、请求 mode、是否等待 ACK、已结束 ID 的有界记录。原生路径保留上游真实 ACK，拒绝事件不能取消另一轮；HTTP 回退只有在能够履行相同合同后才合成控制结果，否则明确 unsupported。`discard_partial_items` 应明确已完成与未完成 item 的规则，并与终态 output、previous-response 缓存一致；已经送出的字节无法撤回，实际产生的用量仍需记录。不要简单把所有 output 清空或伪造上游接受。

验收：缺失/非法 mode、错 ID、旧连接 ID、重复 interrupt、已完成目标、accepted 后终态、failed 后原生成继续、ACK 超时、先有生成错误再中断、HTTP 回退中的部分工具参数及文本。ACK 不占首有效 token、不被 lifecycle 缓冲至死等、不造成双终态/双 usage；所有分支都不自动重开生成。保留现有“生成错误先到则错误优先”回归。

#### R4-CX-05（P2，性能候选）

CLIProxyAPI `064e008b` 在 `internal/runtime/executor/codex_executor_request.go`、`internal/api/response_compression.go` 等处加入出站 zstd、协商式下游压缩和 WS 压缩；codex2api 也有 `proxy/codex_request_compression.go`。目标 [request_governance.rs](src/proxy/request_governance.rs) 已支持 gzip/deflate/br/zstd 解码，解码支持不能证明出站压缩已实现。

先测长历史请求的网络字节、CPU、峰值内存、首有效内容和取消延迟，仅在收益明确时引入大小/收益门槛及明确 endpoint 白名单。压缩拒绝回退须满足本仓现有提交和重试预算，不能照搬任意 400 后重发；SSE flush、Content-Length/Encoding、Vary 与解压额度必须有独立用例。此项不阻塞协议正确性修复。

### 19.7 Cursor：保持现有专用实现，补资源与真实验收

参考 OmniRoute `open-sse/executors/cursor.ts`、`cursor/agentEndpoint.ts`、`cursor/builtinToolBridge.ts`，目标对应 [agent_endpoint.rs](src/proxy/cursor/agent_endpoint.rs)、[agent_driver.rs](src/proxy/cursor/agent_driver.rs)、[response_state.rs](src/proxy/cursor/response_state.rs) 以及 [OAuth 客户端](src/clients/oauth/cursor.rs)。

| 方面 | 对照结果 |
| --- | --- |
| 动态 Agent URL | 双方均解析 GetServerConfig protobuf field 27、约束 api5 域；目标额外检查重复字段、URL 结构和大小，不能写成缺 discovery |
| 工具桥接与强制选择 | 目标已有 schema/名称解析、required/named 的提交前约束、`MAX_SEMANTIC_ATTEMPTS=3` 与绝对 deadline；不需要以无限重试换成功率 |
| 会话及 previous response | 目标将已完成 response context 与 parked h2 session 分离；前者已有 10 分钟 TTL、64 MiB 总量、8 MiB 单项、2000 项上限及身份 fencing |
| 认证与多模态 | 目标已有 OAuth/API-key 双 rail、公钥验证/交换、模型查询和图片路径；两条 rail 的真实接受性须分别证明 |

**R4-CUR-01（P1，验证任务）。** 本次范围为 `reviewed_no_new_gap`，不新增假设性的 wire 修复。新增工作的价值在于把取消、工具等待、parked session、h2 关闭与内存释放做成可观察的生命周期，并接入 R4-CORE-02 总预算。使用本地假上游验证客户端断开、超时、令牌更新、工具结果晚到、TTL/容量淘汰均释放资源，旧会话不被下一身份复用。随后分别采集 OAuth 与 API-key 的文本、图片、工具续接回执；缺哪条就保持哪条 `live_pending`，不从另一条推断通过。

### 19.8 Grok：区分 CLI/OAuth 与 Web/SSO，窄化重试研究

参考 grok2api 的代码根为 `backend/internal/`。已对照 `infra/provider/cli/adapter.go`、`application/gateway/service.go`，并检查 `infra/provider/web/sso_build.go`、`web/image.go` 的适用边界；sub2api 重点为 `backend/internal/service/grok_credential_failure.go`、`grok_free_quota_gate.go` 和 `backend/internal/pkg/xai/cli_identity.go`。

目标 [grok.rs](src/proxy/grok.rs)、[grok_cli.rs](src/domain/grok_cli.rs) 和 OAuth/媒体路径已有 turn index、作用域化 reasoning replay、质量观察、固定账号恢复、媒体归属和 cooldown。当前默认 `grok-shell/1.0.13` 是明确选择，不能因为参考使用较新 Build 版本就直接更新。

**R4-GR-01（P1，`differential_first`）。** grok2api `4e220e81` 在实际 turn 重提交时设置 `x-grok-transient-retry`，保持 `Idempotency-Key`；`isBuildTransientRetryStatus` 选择 5xx 并排除 525/526，同时区分空/循环质量错误。目标尚无该新 header，但已有自己的恢复预算。先比较首发、同绑定 5xx 重试、token 刷新、WS/HTTP 回退各自的 turn/idempotency 定义，确认只有真正的同轮重提交才递增，且提交后不重放、取消不重试、TLS 配置错误不伪装为瞬态流量。通过离线报文差分后仍需同一 CLI/OAuth rail 的真实回执；不能假定 Build 与目标 endpoint 的新版本合同完全相同。

sub2api 的 credential snapshot/CAS 和错误作用域值得作为并发反例来源；目标已有代际防护，应先测“旧 token 的撤销错误晚到，不能隔离新 token”，再决定是否缺实现。它的本地估算 95% free quota soft gate 不作为供应商权威额度导入。grok2api 新增 consent_token、Imagine requestId 筛选属于 Web/SSO 路径，不能据此声称目标 OAuth 缺功能。跨账号媒体重试、质量自动封号、强制继续生成和付费探针均不采纳；remote compaction 保持现有真实证据门禁。

### 19.9 Kiro：已有 EventStream/缓存，计量修正继续以证据为先

| 方面 | kiro.rs 参考 | 目标与判断 |
| --- | --- | --- |
| EventStream | `src/kiro/parser/frame.rs` 的长度、prelude/message CRC、16 MiB 上限 | [wire/frame.rs](src/proxy/kiro/wire/frame.rs) 已具备并更早校验 header 长度/prelude；不再提“补 CRC” |
| endpoint/凭据 | `src/kiro/endpoint/{mod,cli,ide}.rs` 区分 CLI/IDE、profileArn 和错误类别 | 目标 [kiro.rs](src/proxy/kiro.rs) 及 OAuth/Registry 已有相关边界；不能引入凭据池切换 |
| 工具流与缓存 | 参考的工具累积、空参数收尾、缓存逻辑 | 目标已有精确空参数恢复；[kiro_prompt_cache.rs](src/proxy/kiro_prompt_cache.rs) 已有 TTL、SQLite、有界 writer 和身份 scope |
| usage | `src/token.rs::input_tokens_excluding_output` 从 contextUsage 总量减输出 | 目标已有 `metadata.tokenUsage > metrics > context/fallback` 优先级及显式零，并把该减法作为 shadow 候选 |

**R4-KI-01（P1，`live_pending`）。** 参考注释认为 contextUsage 包含本轮输出，但这不能覆盖全部 auth kind、region 和 endpoint。目标测试 `kiro_context_output_subtraction_remains_shadow_only_without_live_receipt` 已明确该取舍。计划采集同轮 contextUsage、metadata/metrics、输出估算与上游账户用量的对应关系，区分供应商计量与兼容 token 展示；只有在证明适用条件后才更改 fallback，且不能覆盖显式权威 tokenUsage。

离线继续覆盖 context 缺失/零、输出大于估计上下文、late usage、截断 EventStream、取消、缓存写失败和身份切换。真实验收按现有四种 auth kind × 两个 region 八路独立完成，不能以某一路的观察全局扣减输入。Prompt Cache 不是新待办；未经验证的 shared cache 与 Compact 仍禁用。除了上述计量/真实证据工作，本次未发现新的专用 wire 缺口。

### 19.10 Qoder：共享工具身份优先，重审历史筛选边界

参考 TokenRouter 的 `backend/internal/upstream/qoder/{execute,payload,signature,session,credential_builder,quota_usage}.go` 和 `backend/internal/provider/provider/qoder_*.go`，按其接口文档区分平台执行器、凭据会话和 gateway 输出职责。目标对应 [qoder_runtime.rs](src/proxy/qoder_runtime.rs)、[qoder.rs](src/proxy/qoder.rs)、[OAuth 客户端](src/clients/oauth/qoder.rs) 及 [Provider facade](src/proxy/providers/qoder/mod.rs)。

目标已有 Global OAuth、Global PAT job-token、CN OAuth 三种 rail；COSY 编码/签名、模型/思考控制、目录与会话身份、嵌套 SSE 解包、工具流/usage 聚合、终态和内存边界均已有实现。目标还已在发网前拒绝未声明的指定工具和无工具的 required；不能照搬参考保留全部工具的宽松 fallback。参考平台执行边界可指导未来拆分，但不是重写目标 executor 的理由。

**R4-QD-01（P1，`differential_first`，依赖 R4-CORE-01）。** 目标 `qoder_runtime.rs::normalize_tool_history_with_continuation`（约 1005 行）只保留本轮声明或 continuation 已知名称，约 1093 行记录 `unknown_history_tool_dropped`，随后删除对应 orphan result；现有 `tool_history_filters_unknown_and_orphan_pairs_without_cross_batch_binding` 明确固定了这一策略。参考 `payload.go::QoderMessagesFromChatCompletions` / `NormalizeQoderToolCalls` 不以活动声明过滤有效历史，`NormalizeQoderResponsesToolPairing` 则按 call/result 配对保留。

因此用户在后续轮次移除某工具、但仍提交完整且已配对的调用历史时，目标可能丢掉工具结果上下文。它是已确认的实现差异，但改变现有测试合同之前应先证明 Qoder 接受该历史；不能把普通孤儿结果、歧义 ID 和合法退役工具混为一类。

计划建立“本轮可调用工具”和“已完成历史身份”两张表，沿 Responses→Chat→Qoder 的最终 payload 对照。验证 tools=[]、tool_choice=none、仅 tool_search、命名空间碰撞、并行结果乱序、跨 batch 同 ID、完整历史与服务器 continuation 两种入口。若上游接受，保留已配对历史但不重启工具授权；若不接受，明确返回不兼容或采用经过记录的降级，不通过伪造活动声明来骗过校验。Global/CN 的结果分别记录，参数/结果正文不改写。

**R4-QD-02（P2，能力扩展候选）。** 参考接口支持 CN 标准 `QODER_PAT`；目标 `domain/qoder.rs::QoderCredentialRail::parse` 仅允许 Global 使用 `pat_job_token`。这是 rail 覆盖差异，不是已有 CN OAuth 的缺陷。可在正确性项目完成后研究 CN PAT 交换、刷新、quota 签名、模型目录及身份 fencing，形成独立 Registry/导入/receipt 切片；证据不足前不复用 Global rail 冒充支持。企业专属 `PERSONAL_TOKEN`、组织/AK-SK 等不随此项扩张。

### 19.11 CodeBuddy：核对完整客户端身份，保留提示词与刷新语义

参考 cli2api 当前实现位于 `internal/providers/workbuddy/`，不是按旧项目名称猜测目录。已对照 `credential.go`、`headers.go`、`ttfb.go`、`payload.go` 以及 WAF/refresh 相关测试。目标为 [codebuddy.rs](src/proxy/codebuddy.rs)、[codebuddy_runtime.rs](src/proxy/codebuddy_runtime.rs)、[域身份](src/domain/codebuddy.rs)、[OAuth 客户端](src/clients/oauth/codebuddy.rs) 和 forwarder 的 CodeBuddy 路径。

| 参考增量 | 目标当前状态 | 决策 |
| --- | --- | --- |
| 30 秒 response-header 超时并区分 parent cancel | `forwarder.rs` 约 18818 行已有 response-header 超时，随后有首 body/idle 约束；默认值可配置 | 不重复开发 timeout；先测 CN/Intl 长尾再调参 |
| 非认证 refresh 失败不标 login_required | `codebuddy_refresh_rejection`（约 1470 行）已区分 invalid_grant、限流、网络和一般 ProviderRejected | 作为已有能力保留，补用例只针对新反例 |
| CLI 2.161.0、IDE/Product/Session headers、private-data 身份表 | 目标固定审过的 2.142.0；已有 agent/request/conversation headers，缺新增 IDE/Product/Session 组合 | R4-CB-01 先差分，不单独替换版本号 |
| WAF 身份文本清洗 | 参考 payload 规则改 system/developer/assistant，部分规则触及 user/tool/reasoning | `not_adopted`：不能以规避拒绝为由修改调用者语义或工具结果 |

**R4-CB-01（P1，`differential_first`）。** 比较两站的推理、登录/刷新、目录所需身份 tuple，核对 UA、IDE/Product version、session/request ID、origin 和 private-data 的组合。目标 `forwarder.rs` 约 18745 行发送 `X-Private-Data=false`；当前导入/登录合同限定个人账号，这与参考 CLI personal 默认一致，不是企业账号 bug。参考目录路径还可能选择不同于推理的客户端身份，不能从目录更多模型推导推理全部可用。

若新身份被真实两站验证，集中定义受审版本和 header 构造，明确 request ID 与 session ID 的生命周期，先兼容旧 profile 再迁移，不让存量账号因版本常量变化全部失效。验收包含 Intl/CN、身份相关头的路径作用域、刷新前后、model allowlist、新旧目录隔离、11128 诊断、11148 历史修复、终态后 usage 和取消 drain。回退须整套恢复身份 tuple 与目录证据，不能仅退 UA。v4.1 等未验证模型仍按现有能力门禁处理；错误分类和固定 prompt 保真优先于 WAF 字串替换。

### 19.12 横向工程改进与明确不迁入的行为

**R4-CORE-03（P2，先测量再拆分）。** TokenRouter `docs/architecture/gateway_request_lifecycle.md` 以及 `8eb3e4343` 的性能存储变更，将入口/准入前准备、排队、上游事件、有效内容和成功交付区分。目标已有 `first_token_ms`、审计记录、Claude TTFB 和内存指标，因此不能写成“没有可观测性”。应先确认不同 Provider 的计时起点和首内容定义，再补一致的 request/attempt 里程碑：准入完成、请求发出、响应头、首语义内容、首成功下游写入、终态、资源释放。SSE 心跳、WS metadata、重试等待和取消 drain 分别归类；这些指标不参与推理成功或 token 计算。

建议用相同假上游工作负载比较 p50/p95/p99、请求分配/保留字节、进程 RSS、取消释放时间及读循环写入阻塞；不写未经测量的“降低 30%”目标。指标标签仅使用低基数 Provider/transport/outcome，不放 token、工具参数、session ID 或用户文本。先修 R4-CX-03，再用更可靠的语义指标评估压缩与调参。

目标当前代码量如下，仅用于定位维护成本，不以行数作为功能缺陷证明：

| 文件 | 总行数 | 文件内主要 tests 模块开始行 | 改造建议 |
| --- | ---: | ---: | --- |
| `src/proxy/forwarder.rs` | 57,373 | 29,703 | 先抽新的 WS 控制状态、配额 observer 和完成处理，保留主编排接口 |
| `src/state.rs` | 34,561 | 22,377 | 沿既有域方法提私有实现，不把直接存储写权限下放到 Provider |
| `src/proxy/transforms.rs` | 11,850 | 7,432 | 工具身份/usage 规则独立成小模块，避免两种协议各写一套 |
| `src/proxy/stream_transforms.rs` | 10,454 | 6,235 | item 阶段、工具状态与终态契约保持清晰，不合并成通用 JSON 猜测器 |
| `src/proxy/cursor/agent_driver.rs` | 7,739 | 5,035 | 只沿可验证的会话/工具等待/完成边界拆，不替换 h2 实现 |

已有 `src/proxy/providers/*` facade 和 `src/proxy/execution/{context,recovery,transport,terminal}.rs` 可以承接切片，不从零“插件化”。纯结构重排应与协议行为提交分开；每片保留同一 BindingSnapshot、CommitGuard、attempt/elapsed/memory 预算与一次 usage 完成。状态修改走 `state.rs` 域方法，遵循 config → providers → accounts → usage → shares → ui_settings → sessions → oauth_logins 锁序；不引入 domain→proxy 或 proxy→clients 的逆向依赖。

| 不迁入项 | 原因 / 可保留的部分 |
| --- | --- |
| 参考项目的账号池、跨账号/Provider 故障转移、并行试号 | 与 Share 固定 Provider/Account 身份不符；只借鉴错误分类和同绑定有界恢复 |
| 输出思考/工具/正文后自动“继续”、拼接另一轮结果 | 会改变已提交结果、请求次数与用量；只保留诊断 |
| 静默 required→auto、任意坏工具丢弃、提示词/WAF 身份改写 | 会改变用户意图；优先精确校验、明确不支持或有证据的窄修复 |
| 伪造 thought signature、混用 commentary 与真实 encrypted reasoning | 破坏签名和历史作用域；内部语义不能冒充供应商凭证 |
| 按版本号高低自动更新客户端指纹、将另一 rail 的目录视为权威 | 参考版本不等于目标账号/站点可用；完整身份与证据一起验证 |
| 本地估算额度直接触发供应商级封号/财务行为 | Server 统计 token、状态和延迟，不承接 Router 的计价、余额与资金结算 |
| 外仓作为日常构建、测试、运行时依赖，或整体迁入桌面/UI/管理台 | 只允许本次只读研究和本仓独立实现；未来 CI 不依赖这些外部 checkout |

### 19.13 可执行的实施顺序、依赖与退出条件

**R4-EVID-01** 贯穿各切片：将本轮真正采用的 source commit、文件/函数、静态反例与决策追加到本仓证据，不覆盖历史 observation。新测试使用本仓脱敏 fixture/假上游，不执行外仓测试作为本仓验收，也不让参考目录成为 CI 输入。协议变更时同步 `PROTOCOL_EVIDENCE.md`、Provider 文档及合同源，再通过生成脚本更新 coverage；本轮只做计划，未提前改这些权威材料。

| 顺序 | 切片与依赖 | 可 review 的交付 | 完成条件 |
| --- | --- | --- | --- |
| A：证据与反例 | R4-EVID-01，按每个候选启动 | 最小输入/期望、当前实际路径、引用提交、状态和不采纳说明 | 能在本仓独立复现；区分已有通过与新失败，不能只有参考仓测试截图 |
| B：工具身份 | R4-CORE-01，最高优先 | 活动声明冲突拒绝；独立历史映射随后一片 | 碰撞换序均不误执行；零发网拒绝；普通工具往返、流式/非流式一致 |
| C：窄范围正确性 | R4-CL-01、R4-AG-01，可独立推进 | usage 方言归一；目录版本快照/CAS | 既有总 token 不变；旧版本/旧 token 均不能覆盖新证据 |
| D：资源父预算 | R4-CORE-02，先于引入新长期缓存/队列 | 可观察父预算→活跃请求准入→跨请求保留对象归属 | 并发总量受限，失败/取消/淘汰无泄漏，无 Provider 健康误处罚 |
| E：Codex 控制与观察 | R4-CX-03→R4-CX-04；R4-CX-02 复用精确 frame 分类；R4-CX-01 独立契约切片 | metadata/control 分类、interrupt 状态、quota observer、phase 往返 | native WS 与 HTTP fallback 均有反例；不放宽 create 后重放边界；不双计/双终态 |
| F：差分后决定 wire | R4-AG-02、R4-CL-02、R4-GR-01、R4-QD-01、R4-CB-01；Qoder 依赖 B | 最终报文/两轮状态差分与采纳或拒绝记录 | 有本仓失败证据才改代码；需真实接受性的项目保持 gate 直至回执有效 |
| G：验证与可选增量 | R4-CUR-01、R4-KI-01、R4-LIVE-01；之后 R4-CX-05、R4-CORE-03、R4-QD-02 | rail 验收记录、性能对照、必要的独立结构/能力切片 | 每项单独退出；无真实凭据不阻止 B～E 的离线正确性成果收口 |

建议一片对应一个主要行为变化及配套证据。R4-CX-01 的 phase 载体、R4-QD-01 的历史接受性等若差分不成立，可交付“维持当前行为及原因”，不能为凑齐改造数量而改 wire。参考观察到的新模型、登录方式、SIWC/attestation 或其他大型功能不自动进入本轮生产范围。

所有生产切片共同遵守以下完成标准：

- [ ] 记录修复前失败用例或明确的结构/容量测量；修复后同例通过，新增 fixture 可脱离外仓运行。
- [ ] 覆盖声明支持的 App/协议入口、stream/nonstream 和适用 transport；不要求未声明的 Surface 伪装可用。
- [ ] account/provider/share 身份、代际变化、取消、超时、终态/EOF、错误优先级与资源释放有相称验证。
- [ ] 事件输出、token 观察与最终记录各有单一所有者；控制 ACK、metadata 和 heartbeat 不制造业务 token。
- [ ] `cargo fmt --check`、`cargo check`、相关 Rust 回归及仓库要求的检查通过；共享行为改变后运行完整 Rust 与有关后端审计，使用仓库已有 regression stack 配置，不把历史绿灯当本轮结果。
- [ ] Provider coverage/conformance、相关 reference-delta 和文档索引保持一致；引用更新为实际实现提交/fixture，状态最多提升到本轮真实达到的层级。
- [ ] 给出按切片的回退：observer/优化可关闭，身份和缓存失效同步处理；不通过回退重新引入歧义工具、跨账号回退或无界资源。

### 19.14 真实验收矩阵与能力提升边界

**R4-LIVE-01（P1）** 复用现有私有 receipt 入口及 schema，不另造一套“成功截图”标准。回执必须关联精确账号身份、Provider/App、站点/region/credential rail、model、操作、客户端版本、目录新鲜度与 harness revision；日志和文档只保留摘要及脱敏证据。一次基础文本成功不能提升工具、图片、刷新或 compact，也不能替另一 rail 通过。

| 类别 | 必须独立保留的验证维度 | 本轮重点补证 |
| --- | --- | --- |
| Antigravity | `antigravity_oauth` / `agy_oauth`，对应 App/模型和目录身份 | 版本变更后的目录、签名工具两轮；compact 不随目录可见而启用 |
| Claude | 当前合同声明的五项 operation；线程/beta/模型约束单独记结果 | thinking usage、强制工具组合、续线程及 pause_turn，不跨模型推断 |
| Codex | 当前合同的五项 operation；native WS 与 HTTP fallback 分开 | control ACK、phase 往返、quota frames；图片 variant/prewarm 沿原门禁 |
| Cursor | OAuth / API-key 两 rail | 动态 endpoint、工具停放续接、图片、取消释放；避免只验登录成功 |
| Grok | inference / media / remote_compaction | 精确 CLI 身份和重提交语义；Web/SSO 结果不能顶替 OAuth |
| Kiro | 四 auth kind × 两 region 的八路 | contextUsage 与权威用量关系、缓存命中边界；不自动启用 shared cache/Compact |
| Qoder | Global OAuth / Global PAT job-token / CN OAuth | 历史工具接受性、目录、刷新/COSY、真实工具参数；候选 CN PAT 必须另建合同 |
| CodeBuddy | Intl / CN 个人账号两站 | 整套客户端身份、模型目录、工具、refresh 与取消尾部 usage |

本轮没有使用真实凭据或发起真实生成，既有 `live_pending` 全部保持。没有回执的能力保持现行 disabled/conditional 状态。离线实现完成、参考源码审计通过、真实接受、提交态证据是四个不同事实，应分别记录。

### 19.15 本轮分析交付检查

本轮只修改本计划文档，保留旧 §0～18 记录，并在顶部建立当前入口。所有八类都有推荐源、目标实现、已实现基线、差异或无新增缺口结论，以及后续动作；源码引用对应 §19.1 的冻结点，行号用于定位，后续实施时以函数和提交为准。

已运行八个默认 `audit-*-reference-delta.mjs`（Antigravity、Claude、Codex、Cursor、Grok、Kiro、Qoder、CodeBuddy），全部通过；它们校验的是**已有冻结资产与观察记录**，不是本轮新 HEAD 的全覆盖证明。本轮未追加这些资产，也未把默认审计写成 `--check-sources` 的结果。

`node scripts/audit/audit-docs-index.mjs`、`node scripts/audit/audit-provider-coverage.mjs --check` 与 `git diff --check` 通过；另对新增章节的本地链接做了存在性检查。本轮是文档分析，未运行新的 Rust 反例、全量 cargo、UI、部署或真实账号验收，未提交、推送。各 R4 切片仍为待实施/待验证，分析完成不代表增强代码已经完成。

<a id="enhance-r4-implementation"></a>

## 20. R4 逐类实施与验收进度（2026-10-27 起）

用户已要求实施第 19 节全部计划，每次只推进一类反代，选择项自动采用推荐方案，最后做整体 review，并禁止 UI 自动化和部署测试。本节是当前实施记录；第 19 节的“尚未实施”是研究交付时的历史状态。研究目标基线仍为 `e5b9e83ad488448473ad604fb855fbe28dd5f456`，研究文档的未提交改动保留。

| 类别 | 本轮项目 | 当前状态 |
| --- | --- | --- |
| Claude | R4-CL-01、R4-CL-02，附本类 R4-EVID-01 / R4-LIVE-01 | 离线实现、完整回归与本类 review 完成，详见 §20.1；五项真实验收仍 live_pending |
| Qoder | R4-CORE-01、R4-QD-01、R4-QD-02，附本类证据与验收 | 共享身份、已声明历史修复、完整离线回归与本类 review 完成，详见 §20.2；退役历史、CN PAT 和三 rail 真实验收继续 live_pending |
| Antigravity | R4-AG-01、R4-AG-02，附本类证据与验收 | 离线增强、隔离后端回归和本类 review 完成，详见 §20.3；两 rail 真实验收仍 live_pending，并行 Market 改动未计入本类集成验收 |
| Codex | R4-CX-01～05、R4-CORE-02 / 03 的共享入口，附本类证据与验收 | 离线实现、正式测量、本类 review 和主仓增量同步完成；57项 R4、3898项 Rust、1917项后端 Node 及10项本地 smoke 通过，验证范围详见 §20.4；本轮维持现有编码，五项真实 operation 仍 live_pending |
| Cursor | R4-CUR-01，父预算的会话资源切片，附本类证据与验收 | 离线实现、3914 项 Rust / 1917 项后端 Node / 10 项本地 smoke、本类 review 与主仓增量同步完成；两 rail 仍 live_pending，验证范围详见 §20.5 |
| Grok | R4-GR-01，父预算的媒体/replay 切片，附本类证据与验收 | 差分、资源实现、17项R4、完整Rust3931/Node1917/本地smoke10、本类review与主仓同步完成；三个operation仍live_pending，详见 §20.6 |
| Kiro | R4-KI-01，父预算的 EventStream/cache 切片，附本类证据与验收 | 13项R4、完整Rust3944/Node1917/本地smoke10、本类review及主仓同步完成；八路仍live_pending，详见 §20.7 |
| CodeBuddy | R4-CB-01，父预算的目录/流资源切片，附本类证据与验收 | 八项R4、完整离线回归、本类review及主仓同步完成；两站仍live_pending，详见 §20.8 |
| 跨类 review | 回查全部 R4 编号、共享资源归属、错误与终态、证据和检查范围 | 五组资源/取消修复与历史审计校正完成；25项新增Rust和四项新增Node回归、完整后端及本地smoke通过，主仓同步结果见 §20.9 |

真实接受性按 §19.14 的矩阵继续独立验收；缺真实输入的项目明确保留 `live_pending`，不冒称通过，也不阻塞其他可实施切片。八类可离线实施范围及整体 review 已收口；真实回执、主仓并行改动的完整Rust集成、部署验证和列明的可选扩展没有因此变成已完成。


### 20.1 Claude：R4-CL-01 / 02 实施与验证

本类依次运行旧实现反例、修复和相同用例；没有同时实施其他 Provider。三条最初回归在旧生产实现上全部失败：JSON details 只有 thinking 没有 reasoning，SSE 稀疏更新覆盖已观察 thinking，以及 Opus 5.5 forced choice 被接受。测试自身的 Rust 类型推导错误先修正，未将编译失败算作行为反例；修复后的新增五条 `claude_r4` 测试均已通过。

| 项目 | 当前交付与取舍 | 验证边界 |
| --- | --- | --- |
| R4-CL-01 | JSON/SSE 共享 thinking→reasoning 映射；非负整数、0、双名称优先级、稀疏合并；原生 Anthropic 不改写 | Chat/Responses 两种下游，重复终态、cache/总 tokens/provider metering 保真；不新增计费记录 |
| R4-CL-02 | 推荐选择明确拒绝已审核 Opus 5.5 的 any/tool，保留其他模型旧策略和 count_tokens | Messages/Chat/Responses × stream/nonstream 均 400，假上游请求为 0；最终模型映射先于检查 |
| pause_turn 差分 | 保持既有 incomplete/null，不采纳参考 completed，不自动继续 | 复用已有 JSON/SSE 回归，真实续线程接受性仍待补证 |
| R4-EVID-01 | 追加独立 r4Reviews，冻结 source commits/文件 SHA-256；旧 observation/snapshot 摘要不变 | 本轮是 working_tree_fixture_verified，implementationCommit=null，不冒充提交态 |
| R4-LIVE-01 | 五 operation 均以 RUN_REAL=0 核对输入 | 均 blocked_inputs/live_pending，缺 Server/Router token、账号/Provider/Share/模型及 receipt；未发真实请求 |

定向 Rust 5/5，Claude 私有 receipt 离线 Node 测试 252/252；完整 `cargo test` 本轮通过：library 3690 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1。`cargo fmt --check`、`cargo check`、严格 `cargo clippy --all-targets -- -D warnings`、Claude reference-delta `--check-sources`、wire profile、Provider coverage/conformance、UI Provider **静态矩阵**、transform coverage、proxy bridge contract、文档索引、证据脱敏及 `git diff --check` 均通过。本地临时配置、空闲 loopback 端口的 `scripts/smoke/smoke-local.sh` 十项后端 HTTP 检查通过，临时服务退出并清理配置。首次 Clippy 指出新增测试断言应使用 expect_err；修正后严格检查及完整 Rust 回归重新通过。未运行 UI 自动化或部署测试，未提交、推送。

`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 scripts/release-readiness.sh` 本轮实际退出 1、decision=not-ready：Router 跨仓 Token Market 审计在 `frontend/components/i18n/locale-provider.tsx`、`src/market_fx.rs`、`src/market_billing.rs` 报三项合同不一致；这些文件不在当前 Server 改动中。RUN_TESTS=0 本身保留 local-contracts-unverified，缺真实输入仍 blocked；部署测试按用户要求未执行。该汇总不能标为通过，也不据此改写外部 Router 仓库或削弱审计。

兼容性与回退：已知 Opus 5.5 的强制工具请求现在得到本地 400，调用者需明确选择 auto/none 或另一个支持强制工具的模型；Server 不自动切模型。usage 回退只撤销明细字段归一，不能改变 output/total 或记录次数；模型约束回退须评审完整策略，不能引入静默 required→auto。其余模型、原生 Anthropic usage、固定绑定及 count_tokens 不因本切片改变。

本类 review 已核对最终模型映射 → Claude contract 的调用顺序、生成/count_tokens 分离、共享转换原生透传边界、usage 总数及单终态、域依赖方向和状态写入约束。没有发现需要扩大本类生产修改的新增问题。Antigravity 委托合同按用户要求留到其独立类别；五份真实回执、续线程/工具/模型厂商接受性继续待输入。本轮只收口 Claude 的离线切片，八类总目标保持进行中，下一轮接续 Qoder / R4-CORE-01。

### 20.2 Qoder：R4-CORE-01 / R4-QD-01 / 02 实施与验证

本类只推进 Qoder 与其必需的共享工具身份依赖。参考仍为 Qoder/AGENTS 推荐的 TokenRouter；当前根 AGENTS 存在，已加载 project-doc/code-comments，按文档路由只读研究提交 `d15137eb59d4d85bdc27ae563c37fcf9b0edebcd` / tree `df18b022d78c4760a89f5d0c372b5467bbc4c60f`，不沿用早期 skill 不可用或根文件被删除的历史状态。业务源码用 Git 提交对象读取，没有写入、构建、测试或运行外仓。新证据追加为 r4Reviews，八个既有证据区块的序列化 SHA-256 前后相同；旧 observation、snapshot、官方 oracle 与 65/9/7 基线计数未改。

四条共享 bridge 反例在旧实现成功编译后全部失败：活动名称碰撞被接受、退役历史误用当前身份、历史授权未声明/错误 kind 的 selector，以及空声明被历史发现重新激活。另一个旧 Qoder builder 反例证明：当前仍声明 read、但选择 none/write 后，最终 payload 删除已配对 read 历史。最初测试自身的 Vec Display 编译问题已先修正，不算行为红灯。

首轮修复后新增用例 8/9 通过；三 Surface 回环继续在 Gemini 命名选择时失败，定位到通用 Gemini 转换在 builder 之前已按 allowedFunctionNames 筛掉声明。现从原始 Gemini 直接函数声明捕获名称集合，通过 Qoder 专用内部字段传给历史匹配，不从客户端的同名 JSON 字段取值；解析工作集与集合保留字节均计入请求预算。当前可调用工具仍由既有 Gemini 过滤及 Qoder tool_choice 决定。随后 12 条 qoder_r4 定向测试全部通过。

| 项目 | 当前交付与决定 | 验证和保留门槛 |
| --- | --- | --- |
| R4-CORE-01 活动身份 | 共享 `(kind, namespace, original_name)`；namespace/function/custom/内部模拟名称碰撞明确拒绝；完全同身份重复兼容 | 两种顺序、长 UTF-8、Chat/Anthropic/Grok 共享兼容入口；三 rail × stream/nonstream 回环拒绝时 catalog、exchange、refresh、generation 均为 0 |
| R4-CORE-01 历史身份 | 新 `responses_tool_identity.rs` 独立分配历史别名，排序、预留裸名/内部名/当前名、ASCII≤64 字节、分组件 hash 与有界碰撞重试 | 长名、同拼接串、同名不同 kind、历史不进入 selector/响应回映；非法空名字不伪造成新工具；原始参数字符串保真，map/error 字节纳入预算 |
| R4-CORE-01 声明边界 | 只扫描直接 additional_tools/已完成或缺状态的 tool_search_output；tools=[]/null reset 优先 | 不递归采纳参数或结果中的同形 JSON；正常发现与 namespace JSON/SSE 旧用例保留；原生 Responses 请求值保持不变 |
| R4-QD-01 已声明历史 | 完整声明身份与当前可调用工具可见性分开；Gemini 源声明在 subset 过滤前捕获 | 三 rail × Claude/Codex/Gemini × stream/nonstream × none/named 最终解码 COSY 请求保留 read 历史、结果与关联，当前 tools 仍为空或仅 write |
| R4-QD-01 已撤销历史 | 参考保留、目标过滤的差异维持真实接受性门禁；没有伪造活动声明来保留历史 | 新 qoder-r4-history-delta.json 冻结两组 Responses→Chat→COSY 最终 payload 投影，Global/CN 各验证；参考为只读源码观察，未执行外仓，不标已支持退役历史 |
| R4-QD-02 CN PAT | 推荐暂缓开放，不复用 Global rail | 参考 CN OpenAPI jobToken/exchange + personal_token → userinfo → Gateway status，保留 refresh/expiry；缺独立官方包 digest、CN PAT oracle/mutation、Registry/导入/轮换合同及真实 receipt，继续 researched/live_pending |
| R4-EVID-01 / LIVE-01 | 追加三条 R4 review，implementationCommit=null，区分工作树 fixture 与参考研究 | 三条现有 real harness 的 RUN_REAL=0 输入核对均 blocked_inputs/live_pending；不发真实请求，不新增或提升能力 |

本轮定向 Rust 12/12、Qoder Node 离线测试 290/290；最终完整 cargo test 通过：library 3702 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1。首轮完整回归曾有一个 Grok 旧用例仍期待静默删除未声明的指定工具；这与本切片的严格 selector 合同冲突，现更新为 HTTP/WebSocket 均明确 422，并保留合法 function/hosted 选择的断言，再重新运行完整回归。该文件只调整共享合同测试，没有开始 Grok 专用增强。最终 cargo fmt --check、cargo check、严格 cargo clippy --all-targets -- -D warnings、Qoder reference-delta --check-sources、CLI oracle、Provider coverage/conformance、UI Provider 静态矩阵、transform coverage、proxy bridge contract、文档索引、证据脱敏和 git diff --check 均通过。预先构建后，随机 loopback 端口、临时配置的 scripts/smoke/smoke-local.sh 十项后端 HTTP 检查通过，临时 listener 已确认关闭、配置由脚本清理。未执行 UI 自动化或部署测试。

本轮 RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 scripts/release-readiness.sh 实际退出 1、decision=not-ready：外部 Router 的 Token Market 审计仍在 frontend/components/i18n/locale-provider.tsx、src/market_fx.rs、src/market_billing.rs 报三项合同不一致；RUN_TESTS=0 自身还保留 local-contracts-unverified，真实输入不足仍 blocked。不能把手工已通过的本地检查冒充该汇总门禁通过，也没有改写外仓或削弱审计。三 rail 另以 RUN_REAL=0 再次核对，均 blocked_inputs/live_pending，没有真实发网。

本类 review 已核对活动/历史/selector/响应表的权限隔离、native Responses 透传、三 Surface 的声明捕获时机、Qoder batch/continuation 精确关联、请求内存归属与释放、无新增状态写路径或反向域依赖，以及旧 oracle/真实门禁不被离线 fixture 覆盖。回退只能撤销新增历史支持并明确报告不兼容，不能恢复歧义声明或以静默放宽 selector 换兼容；CN PAT 的开放须另过官方 oracle 与独立 rail 验收。未知退役历史保留、CN PAT 和三份真实 receipt 均未计完成；本轮收口 Qoder 可完成的离线切片，八类总目标继续 active。尚未提交、推送，下一类接续 Antigravity。


### 20.3 Antigravity：R4-AG-01 / 02 实施与验证

本轮只推进 Antigravity / Agy 与必要共享转换，没有开始 Codex。按 Antigravity/AGENTS 推荐，只读核对 CLIProxyAPI `1f917e77c4562623aa49e9804d999524151a2f96` 和 Antigravity-Manager `b601f5e7375343343922ccab5e50653bc3450c3b`；tree 与十三个来源文件 SHA-256 已冻结在独立 r4Reviews，业务源码通过提交对象读取，没有写入、构建、测试或运行外仓。

| 项目 | 当前交付与决定 | 验证和保留门槛 |
| --- | --- | --- |
| R4-AG-01 客户端身份 | 新领域模块捕获 version / 完整 UA / generation，贯穿目录 key、实际 HTTP、stale、持久观察、发布 CAS、搜索选择与推理头 | 两 rail、同 token 切版本、A→B→A、旧请求晚到、更新失败、空目录、project/endpoint/rail 隔离；128 项上限与旧 token/incarnation 栅栏保留 |
| R4-AG-01 证据失效 | 旧无版本目录、版本变化和进程重启后的观察为 stale/superseded；旧容量错误不能写成新身份 | 同身份 stale 不刷新持久证据；当前身份可替换旧代际观察；独立 project/privacy/quota 探测保留各自合同；缺目录不等于全模型支持 |
| R4-AG-02 两轮签名 | 客户端省略前置 signed thought/carrier、保留 unsigned calls 时，只给同轮首个匹配调用回填；原思考段优先，签名冲突拒绝 | 两 rail × Claude/Gemini Surface × thought/carrier/call-text-call，共十二组完整 HTTP 两轮；nonstream→stream、历史声明与 NONE、名称/参数/ID/历史变更拒绝；既有 caller/账号代际隔离回归保留 |
| R4-AG-02 流式工具 | 不同 chunk 复用 part 位置且显式 ID 不同时分别保留，连续下游索引为 0/1/2 | 同一显式 ID 的参数分片正例通过；旧无 ID 嵌套参数合并回归通过；完整无 ID 调用与参数分片的歧义继续待独立 wire 证据，不擅改合同 |
| R4-CL-02 委托切片 | Antigravity 委托按自身 Gemini 合同处理 any/named/none 与 thinking，不直接继承原生 Claude Opus 5.5 拒绝规则，也不静默改 auto | 两 rail 的最终报文差分通过；仅证明字段保真，模型 entitlement、thinking/工具组合接受性和 Compact 均不因 fixture 提升 |
| R4-EVID-01 / LIVE-01 | 追加三条 working_tree_fixture_verified review，implementationCommit=null；记录隔离验证范围；旧十三个顶级证据字段摘要未变 | reference-delta 普通及 --check-sources 通过，十八个旧 delta、二十二个不可变 observation 保留；两份真实入口输入核对均 blocked_inputs/live_pending |

旧生产实现成功编译后的行为红灯确认：无版本目录观察误作 fresh、前置 signed thought/carrier 签名丢失、重复 stream-local part 位置的三个不同显式工具 ID 被合成一个。修复后，完整 Claude Surface 两轮用例还曾因测试 Provider 默认映射到 Claude Sonnet 而失败：该模型不进入 Gemini replay。现测试明确绑定 Gemini 目标，并核对最终出站模型；没有把该 fixture 配置问题算作生产缺陷或扩大 replay 模型范围。

验证期间，主工作树出现并行 Share Market 改动，导致一次编译报新增字段未补齐和私有方法调用。全部并行修改均保留，本轮后续检查移至同 HEAD 的独立工作树 `/data/projects/cc-switch-r4-antigravity-review.5qmvu_3k`，带入 Claude/Qoder/Antigravity 修改，对 state.rs 仅带入本类十个 diff hunk。最终核对确认其余相关 Rust 文件字节与主树相同，state.rs 的本类十个 hunk 相同；隔离结果不代表包含并行 Market 改动的整体根工作树通过。

隔离快照最终验证：新增 antigravity_r4 定向 14/14；完整 cargo test 为 library 3716 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1；cargo fmt --check、cargo check、严格 cargo clippy --all-targets -- -D warnings 和 cargo build 均通过。Antigravity Node 离线 harness 165/165；Provider coverage/conformance、UI Provider **静态矩阵**、transform coverage、proxy bridge contract、文档索引、证据脱敏与 git diff --check 均通过。没有运行 UI 自动化或部署测试。

本地 smoke 首轮 cargo run 启动未在三十秒内监听，保留该失败记录；随后核对隔离构建产物来源并校验 SHA-256，以临时启动器让未修改的 scripts/smoke/smoke-local.sh 直接启动该二进制，十项后端 HTTP 检查全部通过。两次均确认临时 listener 关闭、配置清理；日志权限 0600，未输出其中的临时 token。

本轮 RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 scripts/release-readiness.sh 实际退出 1、decision=not-ready。此次外部 Router 静态审计报告四项：frontend/components/i18n/locale-provider.tsx 和 src/market_fx.rs 的退役 endpoint 残留、src/models.rs 缺少 Share Contract v8 reader 常量、src/market_billing.rs 缺少保留的 Market capability。RUN_TESTS=0 还保留 local-contracts-unverified，真实输入不足仍 blocked，部署测试按用户要求未执行。没有改写外部 Router 或削弱门禁；本轮手动通过的隔离检查不等于该汇总门禁通过。

两 rail 的真实输入仍缺 Server/Router token、Share URL、独立账号/Share/Claude+Gemini Provider/model 及私有 receipt；RUN_REAL=0 未发真实请求，继续 live_pending。Manager 模块职责可参考，追加“继续”、二次生成和终态/usage 拼接的 auto_heal 不迁入，也不整体替换本仓 IR。

本类 review 已核对领域依赖方向、状态写入封装、客户端身份贯穿与持久观察失效、版本/凭据/发布 revision 栅栏、已有签名冲突拒绝、并行工具签名归属、显式 ID 与分片兼容、委托/原生 Claude 策略隔离，以及旧证据和真实能力门禁。回退须成套处理缓存、发布、消费和旧观察失效策略，不能只退 UA 或放宽工具约束。可实施的离线切片已收口；无 ID 歧义、两 rail 真实回执与并行 Market 集成验证未计完成。未提交、推送，八类总目标继续 active，下一类接续 Codex。


### 20.4 Codex：控制、观察与父预算入口（离线收口）

本轮只推进 Codex 及其必要共享资源入口。按 Codex/AGENTS 推荐，只读使用 CLIProxyAPI `1f917e77c4562623aa49e9804d999524151a2f96`（tree `00273ce1709eac116a12932c5337ed41a9d449f6`）与 codex2api `3c6adfda702efed67931b2d98a491d72bd9d91b5`（tree `6630ed69c78dd11dfd8bbbc3220155f3df4bfefb`）；实际采纳点及文件摘要单独追加 r4Reviews，旧 observations/snapshot 摘要保持不变，不以参考目录作为构建、测试或运行时输入。实现仍未提交，implementationCommit=null。

| 项目 | 当前工作树行为 | 仍需独立处理 |
| --- | --- | --- |
| R4-CX-03 | 精确形状的 metadata/control 与业务、transport commit、first/idle deadline、生成终态分离；空闲 metadata 不进入下一轮，未知事件继续保守处理，SSE 可见集合不扩大 | 完整隔离回归通过；真实帧与厂商接受性保持独立 live 状态 |
| R4-CX-04 | mode=discard_partial_items、活动 response/物理连接、去重、真实 accepted/failed ACK 与生成独立，30秒 ACK 上限；终态先到不伪造 ACK，超时丢连接且不重放/双结算 | HTTP fallback 明确 unsupported 并继续本轮生成；未实现/未声称 HTTP 端完整取消合同 |
| R4-CX-02 | 新旧 rate-limit 方言与 metadata 额度头白名单；默认 bucket、显式零/false、逐字段观察时间和 credits balance 时间；有界、计入父预算的异步合并/最多三次提交尝试 | 各字段保留自己的新鲜度；权威 quota 查询仍负责恢复，真实厂商帧待 live 输入 |
| R4-CORE-02 入口 | 现有 request reservations 与 Codex observer 共用进程父账本，默认观测、显式部署硬上限；父/子原子增长、sticky 失败、缩容/drop 释放与 aggregate 指标 | 尚未完成全部类别独立缓存的 owner review，也未按真实部署测量选定默认硬阈值，不把此行标为整个 CORE-02 完成 |
| R4-CX-01 | 已实现内部 phase 捕获、Messages item 文本块边界及有界降级指标；Chat/Messages 保留公开文本并明确无法无损回放阶段，原生 Responses 保留原字段；6 项 phase 定向验证通过 | 不采纳 cpa:commentary 私有签名，不把 commentary 变成真实 reasoning；缺标准 phase 载体的有损边界保留 |
| R4-CORE-03、R4-CX-05 的传输验收 | Codex request/attempt 单调里程碑与独立 wait/owner release 已落地，见本节末续接记录；R4-CX-05 仍只有初始 codec 探针，保持现有 wire/压缩配置 | 实际 Rust Server 同负载测量、压缩独立决策及本类完整 review 待做，不开启下一 Provider |

metadata 旧实现的固定反例先出现 4 failed / 2 passed；修复 metadata 后，中断反例出现 3 failed / 6 passed。已有 event/type 冲突拒绝原先就通过，未把它写成新增实现。控制状态第一批 15 项通过后，进一步新增身份变化、配额、并发父预算、共享 owner、上限下降、原子回滚、关闭/溢出/重启及提交失败恢复的用例。身份变化收尾沿用本仓本地 `error` + Close 合同及唯一 interrupted/502 usage，不伪造上游 response.failed；原专项中的错误断言已修正。

新配额投影不保留原始帧或敏感 headers，不修改权威 quota 查询结果、Account cooldown 或本次 token/Share 消耗。发布核对 Provider revision/绑定、账号 incarnation/auth/token generation；逐字段时间防止乱序或缺失字段覆盖更完整快照，weekly-cycle 仍走既有独立观察入口并增加 token 栅栏。禁用、队列满、父预算不足、持久化失败及重启丢失队列都不能阻塞/重启生成；不迁入参考的账号池、cooldown 写法、宽前缀丢帧或 create 已发送后的 HTTP 透明重放。

父预算度量逻辑保留字节而非 RSS；先观测再由部署选择阈值，不复制参考的 128MiB。新环境项、额度观察关闭方式及指标口径已加入 .env.example、配置指南和 Codex 文档，coverage 由生成源更新。Account 新增可缺省的 codexPassiveQuota 字段，其余类别的现有 fixture 只补默认值，不属于开启其他类别的协议增强。

验证范围仍是 `/data/projects/cc-switch-r4-codex-review.15ykc9zh`：带入此前 Claude/Qoder/Antigravity 修改，排除并行 Share Market 修改。最终离线检查见本节末表；该结果不能算作包含并行 Share Market 修改的整个主工作树回归通过。未做 UI 自动化或部署测试，五项真实 operation 保持 live_pending。


R4-CX-05 已补独立、可复现的初始 codec 探针与四组冻结数据：短请求55→64字节，重复长历史976896→4138字节，工具历史1467494→400524字节，高熵base64文本1048625→786942字节。每组51次的3级编码CPU中位数分别约0.004、0.187、7.519、1.402ms。此数据只来自 Python 3.10.12 / 宿主 libzstd 1.4.8，测量时有其他编译/测试负载；不是 Rust Server 基准，也没有测真实网络、首语义内容、取消或部署RSS。压缩后逐字节回环一致，输入与探针摘要已冻结；仍不启用新出站/下游/WS压缩。R4-CORE-03 的时间里程碑、实际 Server 同负载对照、endpoint/提交后重发边界及部署硬预算选择继续待做，不能把初始 codec 数字当作本类性能项全部完成。


R4-CX-01 的两条新增反例在旧实现成功编译后均失败：SSE 把不同 message 合成单文本块（JSON 则有两个块），且 Responses→Messages 的长期转换状态 retained_bytes 为零。修复保留本仓既有 per-item delta 去重，增加完成集合/当前 item 边界及阶段捕获；不宣称从零补每 item 去重。未知 phase 不进入动态标签或长期原字符串，真实 encrypted reasoning 继续走已有认证封装。6 项 phase 专项全部通过；其中原生透传、缺失/未知阶段、稀疏及冲突、多 item/工具/真实 reasoning 和历史降级均有独立断言。新增原生 SSE fixture 曾误用不存在的 transform_chunk 方法，已改为实际 push 入口后编译通过；该编译错误不算行为红灯。完整回归成绩见本节末表。

第一轮完整 Rust 回归为 library 3739 passed / 1 ignored、API 123 passed / 1 failed；失败是非活动账号测试期望零出站但计到一次连接。带请求目标诊断后的同一用例、24 次并发复跑与 API 124 项复跑均通过，尚未捕获再次失败的具体目标。源码追踪发现 setup 会启动一个同样使用 http_client 的 Router 隧道任务；本测试改用同一 setup 域入口并显式 start_client_tunnel=false，在安装计数代理前即不启动该无关后台任务，保留三条账号动作的冲突状态、错误码与零连接断言，并保留仅记录请求首行的诊断。此处是测试隔离修正，不能把未再次捕获的连接说成已确认的厂商账号请求或已确认的 Router 连接。


Codex 独立保留对象的 owner review 确认如下缺口，现已补齐本轮离线实现。9 项新增资源用例与 phase、metadata、interrupt、quota、父/子预算等合计 39 项 R4 专项全部通过；完整回归结果见本节末表。不能将本表当作所有 Provider 独立缓存或真实部署 RSS 已完成：

| 对象 | 保留的现有界限 | 当前父预算实现与验证 |
| --- | --- | --- |
| `codex_models.rs::ManifestModelCache` | 最多 4 个身份 scope，不跨账号/代际借用能力 | 模型/能力字符串按容量保留 owner，索引固定容量先准入；替换、淘汰、drop 释放。可选缓存拒绝保留最近有效身份快照，不改变当前推理健康 |
| `previous_response_cache.rs` | 10 分钟 TTL、64 MiB 编码字节、2000 entries；8 MiB / 200 items 每条；有界 tombstones | 独立核算 JSON 分配容量、scope、索引估值、tombstone，另报 retained 字节；无额外完整序列化副本的 counting writer 维持编码上限。TTL/drop 释放，父拒绝保持 required-context strict miss；tombstone 预算不足可省略诊断 |
| `ResponsesUpstreamWebSocket` 的 idle read queue / cached socket | 128 帧队列及原连接数/idle/max-age 上限 | idle 帧向父预算预留，active 帧沿用请求 owner；已排队帧不随 turn 变更重复计费或提前释放。wrapper/通道槽位/池索引另有逻辑估值，actor 持共享 owner。满队列等待的第 129 帧、abort、peer close、TTL/容量淘汰、acquire 和容量失败均验证释放 |
| referral `SessionClientPool` | 64 会话、每 host 最多 2 个 idle 连接 | key/entry/索引 owner 随池及进行中的 client clone 持有，淘汰不提前释放；最后 clone drop 归还。cookie/TLS/HTTP 内部占用不透明，不用逻辑估值声称 RSS 全覆盖 |

以上仍属于当前 Codex 类别；其他 Provider 的专用缓存留到各自类别逐项 review。没有用默认关闭、直接删除已有缓存或放宽跨身份复用来绕过这些验收。


资源专项开始前的严格 Clippy 因 phase 扩大后的 GeminiOpenAiTarget::Chat 枚举尺寸差异失败；已对 Chat 状态增加间接存储，并将这份新 heap allocation 纳入 retained bytes，不用 allow 掩盖告警。同时发现 Responses→Gemini 复用的 Responses→Messages 中间状态仍被旧分支计为零，现与原 Messages bridge 共用同一 retained_bytes，phase 预算用例已覆盖两条路径。当前 39 项 R4 专项包含这项回归。


本轮源码冻结后完整 `cargo test -- --test-threads=8` 已退出0：library 3754 passed / 1 ignored，API 124 passed，Cursor fixture 与 Router lease 各1 passed，doc-tests 0项。后端 Node 全套1910/1910通过；这不包含 UI 自动化。最终版本的严格 Clippy、cargo check、构建、格式检查及本地 smoke 也已通过，成绩见下表；没有使用 phase 之前的旧成绩替代最终代码验证。

前一轮四类 owner 切片 review 时，已区分“长期缓存 owner”和当时尚待收敛的临时路径：`src/image_store.rs::ImageCapabilityStore` 自身只保留路径，`StoreSnapshot` 是每次扫描的临时对象，256 MiB 是磁盘 payload 总量上限；`get_at` 仍在 `fs::read` 时读入整个 payload，GET/HEAD 共用该路径，返回的 Bytes 生命周期尚无独立父 owner。`forwarder.rs::inject_previous_response_context` 的 lookup clone、清理/合并临时 JSON 以及 WS 再序列化也需要沿请求预算明确所有权和峰值。后续在当前 Codex 类别处理这些入口，保留图片完整性/跨进程锁、HEAD/GET 合同及 required-context miss，不将本轮四类长期对象的绿灯写成全部图片/缓存临时分配已覆盖。R4-CORE-03 和实际 Server 对照测量仍未完成，不开始下一 Provider。


R4-CORE-03 后续实现须保持既有成功/用量/重试语义：统一起点使用本地单调时钟，request 与 attempt 分开，另记录 retry wait；不能更改现有 first_token_ms 的统计口径来制造改进。HTTP 的 Body poll/yield 只能称 body handoff，不能据此声称 socket 成功写入；WS 的真实 writer completion 与保守提交决策也要分开：`send_responses_upstream_while_serving_downstream` 在 peer-terminal 与 write-error 竞态时可返回 Sent 以禁止重放，这不证明写入实际成功。语义首内容继续排除已审核 metadata/control/keepalive，终态、取消 drain 和最终 owner 释放分别记录。指标只能使用有界 Provider/surface/phase/result 标签，不使用用户、session、响应 ID、工具参数或任意模型字符串。此段是设计约束，尚未把里程碑功能标成实现。


本切片最终离线验收如下。313 个 Rust 文件在最终编译前冻结摘要，各步骤前后均一致；严格 Clippy 中间还曾发现新测试夹具的多余 `.into()`，删除后对最终源码重新运行全目标 Clippy 和完整 Rust 回归，两者均通过。

| 检查 | 实际结果与范围 |
| --- | --- |
| `cargo fmt --check`、`cargo check` | 全部退出0 |
| `cargo clippy --all-targets -- -D warnings` | 最终版本退出0；没有增加 allow 屏蔽告警 |
| 完整 `cargo test -- --test-threads=8` | library 3754 passed / 1 ignored；API 124 passed；Cursor/Router lease 各1 passed；doc-tests 0项；合计3880通过 |
| 后端 Node 全套 | 1910 passed / 0 failed；不含 UI 自动化 |
| `cargo build` 与本地后端 smoke | 本隔离树实际构建退出0，核对 `.d` 来源和源码摘要后复制独立产物；smoke 原脚本通过临时 cargo launcher 启动该产物，10项HTTP检查通过，listener和临时配置均已清理，含临时token的日志仅留0600文件 |
| Codex reference-delta `--check-sources` | 18 deltas、22份旧 immutable observations、5份 R4 working-tree reviews 与5项 live_pending operation，通过；旧证据未重写 |
| 本仓静态合同 | conformance、Server Provider、Phase0、coverage、UI Provider 静态矩阵、transform、7项 bridge、docs index、redaction、`git diff --check` 全通过；生成文档由源生成 |
| 离线 readiness | `RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 实际退出1，decision=not-ready / verificationState=failed，未做部署测试或真实请求 |

readiness 的本地合同失败来自只读外部 Router：`frontend/components/i18n/locale-provider.tsx`、`src/market_fx.rs` 存在退役 Token Market 路径，`src/models.rs` 缺 Share Contract v8 reader constant，`src/market_billing.rs` 缺保留的 Market capability。另缺8项真实输入，`RUN_TESTS=0` 的本次 readiness 调用自身仍报告 local-contracts-unverified，用户禁止部署测试故保留 deployment-not-tested；不能用上表独立执行的后端绿灯改写该命令的结果。未修改外仓、未削弱审计，五项 Codex 真实验收继续 live_pending，implementationCommit 继续为 null。本轮临时内存路径、CORE-03/实际 Server 测量和 Codex 本类完整 review 尚未完成，总目标保持进行中。


本切片的49文件 Codex 增量已同步回 `/data/projects/cc-switch-server`。补丁来自实施前文件快照→隔离树，先检查后应用；没有使用相对HEAD的整份diff或整文件覆盖。与并行 Share Market 重叠的 `model_health.rs`、`router_contract.rs`、`state.rs`、`tests/api_contract.rs`，逐文件核对为“同步前主仓内容应用该增量”的精确结果；其余838个主仓文件摘要不变，两个Phase0摘要明确排除后从主仓当前源重新生成。上述隔离 Rust/Node/build/smoke 绿灯仍不代表并行 Market 改动已经整体回归。没有提交、推送、UI自动化或部署测试；当前继续保持 Codex 类别，后续入口见上文临时内存与性能待办。


#### Codex 图片下载与 previous-response 工作集（续接切片）

基于上一轮同步后的新快照继续，仍只推进 Codex。`ImageCapabilityStore` 现在除 root 外持同一进程父账本句柄，不在内存长期缓存图片；256 MiB 仍是磁盘总量。GET 先核对 metadata/文件长度，再预留 payload 与 owner，按固定长度读取并以额外一字节探测拒绝越界，最后验证 SHA-256。Bytes 通过共享 owner 绑定内存，clone/slice 最后释放才归还；父预算不足走既有本地 capacity 503，不当作文件损坏或 Provider 故障。HEAD 仍验证全图 SHA，只循环使用最多64 KiB buffer，保留原 Content-Length、MIME、空 body 及 Router Share 鉴权合同。跨进程文件锁、TTL/淘汰和原子落盘均保留；取消不能强制中止已经启动的 blocking 文件任务，但其完成结果被丢弃时会释放 owner。

previous-response 新增 domain/state 的同步借用回调，proxy 在锁内先预留临时工作集再按原白名单清理/复制工具项；不再先深拷贝缓存 Vec 或当前 input。当前 input 移入合并向量，opaque call ID、完整身份 scope、敏感字段清理和去重合同保留。HTTP/WS 解析前准入保守结构估值，成功后调整到 retained bytes 加工具配对索引 allowance；lookup 前另预留完整 scope 临时 key。再序列化前计精确长度并一次分配。HTTP 输出转入现有 Bytes owner，WS 输出预留移交 normalized turn owner，不重复记同一 wire。缓存存在但本次工作集不足返回 capacity，不能增加 required-context-unavailable 计数或开启重放；真正缓存/身份 miss 继续严格处理。该实现不把 JSON/索引估值称作精确 RSS。

新增7项生命周期/容量用例与原有39项合计46项 Codex R4 专项在最终源码上全部通过。完整 Rust 回归为 library 3761 passed / 1 ignored、API 124 passed、Cursor fixture/Router lease 各1 passed，总计3887 passed；后端 Node 1910 passed / 0 failed。最终 cargo fmt --check、cargo check、严格全目标 Clippy 与构建均退出0。313个 Rust 文件在最终编译前冻结，编译过程中无源码变化；补齐 lookup key 与配对索引 allowance 后重新执行了完整回归，没有以第一轮成绩代替最终验证。

本地 smoke 原脚本使用最终构建的独立副本，10项HTTP检查通过，listener/config/launcher 均清理。为减少构建磁盘占用，smoke 副本仅去除调试信息，并逐个核对 ELF 全部带 SHF_ALLOC 的节的地址、大小、类型和内容摘要保持一致；原构建和副本摘要分别保留，不冒称两者文件字节一致。临时日志为0600，未将token输出到交付文档。Codex reference-delta --check-sources、本仓 conformance/Provider/Phase0/coverage/transform/bridge/docs、UI Provider 静态矩阵、redaction 与 diff 检查通过。

离线 readiness 实际使用 RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0，仍退出1 / not-ready：同一外部 Router 四处合同问题、8项真实输入缺失、该调用自身 local-contracts-unverified 及按约束未做部署测试分别保留。独立后端绿灯不能改写其结果；五项真实 operation 继续 live_pending。R4-CORE-03 统一里程碑、实际 Rust Server 同负载的延迟/取消/阻塞/RSS测量、R4-CX-05 独立决策和 Codex 本类完整 review 仍未完成，不进入下一 Provider。


#### Codex 转发时间里程碑与请求副本（续接切片）

R4-CORE-03 已增加独立 `request_timing.rs`，在 Codex HTTP/WS 的生产转发路径观察 request/attempt：准入、发送、响应头、首 Business 语义内容、HTTP body handoff、WS 本地发送成功、终态与末次已核算 owner 释放。HTTP 起点是转发入口，已接收的 API body 与鉴权耗时不包含其中；WS 从 `response.create` 数据帧开始。保留原 `first_token_ms` 和成功/usage/重试判定。失败后恢复的中间 attempt 不提前发布 request 终态；入队时捕获 attempt，不让迟到的 writer 结果污染下一次尝试，也不把保守 Sent 当成实际写入成功。没有新增 TokenRouter 计时代码审计，时间实现为本仓独立设计；codex2api 既有冻结来源仅支持 owner 生命周期原则。

operation wait 区分 retry、headers、upstream read/write、downstream write/demand、cancel drain，读取等待不跨 body yield；区间可以重叠，不能相加成总延迟。HTTP handoff 不能声称 socket 写成功，WS sink 完成也不证明远端消费。release 由预算最后一个 owner drop 报告，Bytes clone/slice 或 actor 仍持有时不会提前计为释放；取消 drain 不冒充异步 usage 结算完成。结果只含 complete/incomplete/failed/cancelled/retried/unfinished，未归类异常保持 unfinished；有界指标标签不含用户、模型、session、response ID 或工具内容。

请求准备检查同时收敛三处副本：HTTP 的 opaque previous_response_id 在复制前准入，WS 直接借用 ID；嵌套 response body 用 take 移出，active body 在最后一次借用后移交。保持原 ID 字节、空字符串、平面/嵌套请求与 required-context miss。此项不声称周边所有 policy normalization、parser/library scratch 或图片目录扫描已完整核算。

新增10项回归覆盖 metadata/control/liveness、重试起点与结果隔离、空终态、HTTP handoff、末次 Bytes 释放、取消 drain、完成后的迟到关闭、生产 HTTP 转发/聚合、WS 入队 attempt、opaque ID 容量与嵌套请求抽取。最终完整 Rust 日志确认56项 Codex R4 专项全部通过；完整回归为 library 3771 passed / 1 ignored、API 124 passed、Cursor fixture/Router lease 各1 passed，合计3897 passed。后端 Node 为1910 passed / 0 failed。格式/check、严格全目标 Clippy、完整测试和 build 全部退出0，314个 Rust 文件在格式化后冻结，整个有效编译过程无源码变化。一次在格式化结束前误启动的专项编译已主动停止（143）并排除；静态检查中误写的不存在脚本也已排除，随后仓库实际 redaction 检查通过。

最终构建的独立去调试副本通过原本地 smoke 的10项 HTTP 检查；全部 ELF SHF_ALLOC 节的地址、类型、大小和内容摘要保持不变，原件/副本 SHA 分别留存。listener/config/launcher 均已清理，日志0600。Codex reference-delta --check-sources、conformance、Server Provider/Phase0/coverage、transform/bridge、文档索引、UI Provider静态矩阵和差分检查通过。验证仅覆盖隔离树，不覆盖主仓并行 Share Market 变更；没有UI自动化或部署测试。

本时间切片已以14文件增量同步主仓，874个无关文件在同步期间保持原摘要，两份Phase0生成件按主仓源重新生成；同步时主仓HEAD为075924cb3730eca0a300648a9d4c87e83c2c8dea。主仓重跑上述静态审计全部通过。离线readiness明确使用RUN_TESTS=0 / RUN_REAL=0 / RUN_DEPLOYMENT_TESTS=0，实际退出1、decision=not-ready、verificationState=failed：外部Router四处合同问题、8项真实输入缺失、local-contracts-unverified与deployment-not-tested分别保留。主仓静态绿灯与隔离树后端回归不合并宣称为并行Market的全量集成验证。

仍仅处理 Codex：真实 Rust Server 同工作负载 p50/p95/p99、首语义内容、取消释放、读写阻塞与 RSS 测量，R4-CX-05 独立压缩决策及本类完整 review 尚未完成。新时钟的 fixture 不是性能改进或厂商接受证据，五项真实 operation 继续 live_pending；不进入 Cursor。


#### Codex 实际 Server 探针与回退写入观测（正式采样中）

以下更新接续上文各历史切片；旧段落中的待办和旧验证成绩保留其当时范围，当前状态以本段及后续正式收口段为准。新增 `src/test_support/codex_server_probe.rs` 和 `scripts/bench/codex-server-probe.py`，仅在 Linux 的独立、默认 ignored 测试进程装配真实 `api::app_router`、TCP listener、正常 v2 签名 Share ingress 和固定合成 CodexOAuth 绑定。HTTP/WS endpoint override 只使用既有 cfg(test) 入口；在启动 API listener 前，核对 runtime plan 的 scheme、127.0.0.1、精确临时端口及无 credentials/query/fragment。没有新增生产任意 endpoint 开关，也没有引入 proxy→api/clients 依赖。

四条路径是 native WS、WS 的 HTTP fallback、HTTP SSE 和 HTTP JSON 聚合；四种输入复用旧 codec 探针的 short、long_history、tool_history、base64_entropy 生成器及原始输入摘要。另有三条流路径的取消与慢读，合计22行。准备输入包含 stream 选择，WS 的 response.create envelope 在其后添加；实际送达假上游的规范化应用字节另计，两者均不含 transport framing、headers 或 TLS。正式采样采用101次/项，慢读31次/项，各排除5次 warmup；分位数按 nearest-rank 计算。每次要求仅一次上游 generation，并等待末次 request owner 释放及 high-water 回调后再清 warmup。

慢读输出为8 MiB、每个客户端 chunk/frame 延迟5ms；使用16个已完成512 KiB message item，再由既有稀疏 terminal 重建，保持每个 HTTP SSE 未完成事件低于2 MiB，未放宽128 MiB完整事件或其他生产限制。HTTP 取消丢弃 body；WS 明确发送 Close 并等待有界 peer closure 后丢弃 socket。Server cancel_drain 与客户端返回后观察到 owner release 是不同起点；HTTP 首到达字节与 Server first_semantic 也分别记录。

前期试运行边界均保留：错误注册 /responses 已改为 /v1/responses；8 MiB terminal 的单行、随后多行形状都不满足既有整个 pending-event 的2 MiB上限，多行并不能绕过该限制；立即 Close/drop 曾得到2次cancelled和1次failed，后改为等待关闭握手，未改生产分类。另一次 settingsConfig 中的测试 WS URL 未进入 runtime plan，可能回退默认厂商地址，已停止并排除该轮，不能把它记作“零可能厂商连接”或 live/性能证据；未使用真实用户凭据。最终装配直接安装测试 runtime plan，并在 API listener 启动前校验回环端点。

实际小样本还暴露一处生产观测遗漏：六条 fallback_ws 行均缺 downstream_write 和 downstream_write_completed。红灯记录来自当时实际 Server 报告，而非猜测。`relay_codex_http_fallback_event` 现在只对实际 sink await 计等待，Ok 后记写入完成，保留原来的错误返回和生成/用量/重试语义。新增普通回归通过真实 TCP WS 验证交付、等待/完成标记，并确认单次发送不会制造 first_semantic、terminal 或成功结果。两条 WS 的探针也增加对应断言。

最终生产代码与探针上，315个 Rust 文件格式化后冻结；cargo fmt --check、严格全目标 Clippy、cargo check、完整 cargo test 和 cargo build 均退出0，编译期间源摘要未变。完整 Rust 为 library3772通过/2忽略、API124通过、Cursor fixture及Router lease各1通过，共3898通过/2忽略，其中57项 Codex R4 专项通过；独立小样本22行全部通过。新构建的独立 strip-debug 副本逐个核对全部 ELF SHF_ALLOC 节不变，原本地后端 smoke 的10项HTTP检查通过，listener/config/launcher均清理，含临时token的日志保留0600。

正式101/31次采样正在运行，当前没有正式分位数或最终数据审计成绩。RSS采样覆盖整个debug测试进程，包含Server、假上游、客户端、collector、1ms sampler及allocator retention；逻辑账本不是allocator分配计数或RSS，顺序负载也不是生产容量或并发部署压测。普通生产后台调度及Router tunnel未启动；没有前后提速声明。新数据合同的后端Node、reference-delta和文档审计仍待正式资产冻结后执行，不能以较早的1910项Node成绩替代。验证继续只覆盖隔离树，不包含主仓并行Share Market修改；五项真实operation保持live_pending，没有UI自动化、部署测试、提交或推送。


#### Codex 正式 Server 测量、压缩决定与本类 review

正式报告已冻结至 `assets/contract/codex-r4-server-probe.json`，采集时间 `2026-10-27T15:10:55.508414532+00:00`。四路径22行全部完成：19条普通/取消行各101个 measured samples，三条慢读行各31个，合计2012次 measured generation；另有110次 warmup。每次恰好一次上游生成，request owner release 和 high-water 回调均达到预期，两个WS路径均有真实下游写入等待/成功观测；warmup不进入分位数。最终driver记录314个Rust/构建输入文件的聚合摘要及三个探针文件的摘要，测量前后均一致；外层runner另冻结315个Rust文件。测试二进制摘要、工作树HEAD、主机/编译配置与清理结果均在资产中，没有把未提交实现写成提交态。

下表是同一debug装配的实测基线，所有延迟均为毫秒，单元格顺序为p50 / p95 / p99。Server起点是已完成API接收/鉴权后的转发入口或WS数据帧接收；客户端WS turn起点不含握手，HTTP客户端起点包含入口处理。HTTP首到达可只是启动字节，不能替代本表的首语义内容；这些时钟也不能直接相减归因。

| 路径 | 输入 | Server首语义内容（ms） | 客户端完成（ms） |
| --- | --- | --- | --- |
| native_ws | short | 24.510 / 26.191 / 27.744 | 40.277 / 42.879 / 43.731 |
| native_ws | long_history | 281.681 / 314.307 / 316.335 | 358.120 / 402.029 / 403.242 |
| native_ws | tool_history | 583.926 / 606.775 / 611.272 | 702.385 / 745.245 / 763.004 |
| native_ws | base64_entropy | 249.158 / 272.918 / 282.709 | 327.087 / 356.748 / 367.917 |
| fallback_ws | short | 27.120 / 28.597 / 29.290 | 43.551 / 45.322 / 46.555 |
| fallback_ws | long_history | 830.322 / 845.595 / 855.010 | 903.036 / 918.619 / 927.266 |
| fallback_ws | tool_history | 1654.884 / 1666.941 / 1673.864 | 1765.326 / 1808.362 / 1812.899 |
| fallback_ws | base64_entropy | 768.963 / 784.765 / 825.104 | 842.945 / 858.586 / 899.010 |
| http_sse | short | 8.638 / 9.805 / 10.249 | 84.096 / 88.241 / 91.664 |
| http_sse | long_history | 533.205 / 536.208 / 537.722 | 717.496 / 722.336 / 725.935 |
| http_sse | tool_history | 1117.901 / 1126.632 / 1133.979 | 1375.593 / 1385.668 / 1392.931 |
| http_sse | base64_entropy | 478.816 / 484.164 / 485.661 | 673.277 / 680.405 / 794.893 |
| http_json | short | 23.175 / 25.455 / 26.137 | 47.515 / 49.872 / 50.772 |
| http_json | long_history | 545.328 / 558.365 / 566.444 | 719.633 / 731.188 / 738.951 |
| http_json | tool_history | 1148.251 / 1160.026 / 1168.767 | 1395.008 / 1408.014 / 1418.543 |
| http_json | base64_entropy | 491.796 / 497.933 / 512.000 | 674.976 / 682.351 / 704.103 |

取消测量分别保留客户端调用返回、返回后观察到owner释放及Server明确取消至末次owner释放三个口径，不能把关闭握手耗时或两者的差值解释为生产取消成本。

| 路径 | 客户端返回（ms） | 返回后观察释放（ms） | Server cancel_drain（ms） |
| --- | --- | --- | --- |
| native_ws | 102.194 / 117.318 / 121.525 | 0.059 / 0.075 / 0.103 | 10.725 / 20.034 / 21.453 |
| fallback_ws | 56.209 / 60.184 / 62.282 | 0.058 / 0.089 / 0.094 | 4.566 / 5.121 / 5.276 |
| http_sse | 20.314 / 22.411 / 24.250 | 3.165 / 4.450 / 4.673 | 0.133 / 0.205 / 0.254 |

8 MiB慢读仍按每个客户端chunk/frame延迟5ms；HTTP chunk边界由传输决定，WS按frame延迟，因此不同路径的客户端完成时间不能直接作为协议优劣结论。31个样本的nearest-rank p99就是该行最大值。

| 慢读路径 | 客户端完成（ms） | Server首语义内容（ms） | 整进程采样峰值RSS（MiB） |
| --- | --- | --- | --- |
| native_ws | 1689.347 / 1716.930 / 1781.334 | 19.544 / 51.044 / 151.501 | 279.88 |
| fallback_ws | 3808.549 / 3844.502 / 3848.216 | 556.684 / 572.877 / 576.187 | 289.90 |
| http_sse | 5357.623 / 5396.810 / 5398.935 | 553.949 / 558.125 / 560.550 | 318.34 |

全部行的整进程采样RSS峰值为318.34 MiB，采样逻辑保留量峰值为8415379字节。RSS包含Server、假上游、客户端、collector、1ms sampler和allocator retention；顺序执行、debug代码、warmup及前面各行会影响后续RSS，不是独立生产Server的容量或前后提速证据。请求owner释放后仍可能有物理socket/actor父账本owner，当前非零行记录为`[["native_ws", "short", 20104], ["native_ws", "long_history", 20104]]`；后续物理owner释放与request owner释放不同，不能声称每一行父账本均为零。没有allocator分配计数，采样也可能漏掉更短峰值。普通后台调度和Router tunnel没有启动，进程父预算维持默认观测模式，不能据此设置部署硬阈值。

R4-CX-05本轮决定为`keep_current_encoding`：HTTP出站、下游和WS编码保持现状，候选本轮关闭。CLIProxyAPI的出站压缩有端点/身份/大小/收益门槛，下游另处理协商、Vary和SSE flush，WS另协商扩展；codex2api采用不同默认值与路径策略。codec压缩率与当前未压缩Server基线都不能证明厂商接受、网络收益、flush安全、codec scratch预算或WS协商。因此没有引入新压缩、通用400/415后明文重发或生成重放；未来启用必须重新具备相同端点/rail的真实接受证据、owner预算和相称的传输对照。

本类review逐项核对CX01～05及Codex的CORE02/03入口：phase在标准协议的有损边界明确，metadata/control不制造业务内容或成功，中断ACK独立于生成终态，被动quota不写token/健康冷却，身份/代际栅栏、固定绑定、create发送后不透明HTTP重放、缓存/临时工作集owner和实际写入计时保留各自所有者。未新增TokenRouter计时业务源码研究，时间模型是本仓独立设计。旧18条source delta、22条不可变observation及历史codec资产不覆盖；新增第七份R4-CX-05 review和本类working-tree review，implementationCommit仍为null。五项live operation仍live_pending；其他Provider的独立缓存、计时范围和最终跨类review继续待做。

上文的正式采样中、旧Node成绩和旧待办均为当时记录。本段关闭实际Server测量与本轮压缩决策；最终数据合同、当前后端Node、静态审计及保护并行Share Market的主仓增量同步结果在下文追加，不以隔离树完整回归替代并行修改的集成验收。没有UI自动化、部署测试、提交或推送。


正式资产冻结后的最终后端审计：新增纯数据合同5/5，当前完整后端Node为1917/1917、0失败。第一次全套运行是1914通过/1失败：产品边界审计没有继承src/lib.rs中cfg(test)外置模块的条件，误将探针客户端归为生产构造器。只修审计及其Node回归，不改已测Rust/driver/fixture：仅对声明式crate root的显式测试模块识别继承，出现生产模块/路径/include引用则撤销排除；注释、字符串、未知path属性不获得豁免，目录名本身不授权。新增两项回归包含上述正反例，最终全套重跑通过；生产代码绕过统一HTTP构造器的拒绝继续有效。

最终14项数据/后端/静态检查均通过，包括Codex reference-delta --check-sources、conformance、产品边界、Server Provider、Phase0/coverage、transform/bridge、文档索引、UI Provider静态矩阵、脱敏与差分；另核对五项状态写入/依赖方向检查。独立重建全部22行prepared JSON并核对字节数/摘要，也核对四个原始codec输入；正式报告SHA-256为b6d6d2d9a296c3a02e1df00c98891e584f0b8fd604f92e399caa87e2e23fb53d。315个已冻结Rust文件在这些后端检查中继续不变；没有为JS审计改动重复宣称新Rust编译或新的厂商接受。17文件主仓增量预检无重叠，实际同步及主仓离线readiness结果待下文记录。


Codex 主仓同步与离线收口：按post-timing基线重新生成17文件增量，预检及应用均无重叠，876个无关文件逐项摘要不变，没有新增无关文件；Phase0两份生成资产从主仓源码重新生成。同步时主仓HEAD为075924cb3730eca0a300648a9d4c87e83c2c8dea，这不是隔离树回归的e5b9e83基线。主仓cargo fmt --check及12项相关静态/源码/文档检查全部通过，文档索引为51文件/146链接，核验期间Rust源码未变。没有在主仓重新运行包含并行Share Market改动的完整Rust回归，3898/1917/10等完整成绩继续保留原隔离范围。

本轮实际RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0的离线readiness退出1、decision=not-ready。外部Router合同仍有四处失败：frontend/components/i18n/locale-provider.tsx及src/market_fx.rs含退役Market端点，src/models.rs缺Share Contract v8 reader constant，src/market_billing.rs缺保留Market能力；同时缺八项真实输入，local-contracts-unverified和deployment-not-tested仍阻塞。未修改Router或放宽门禁。Codex本类离线状态收口为offline_complete，五项真实operation继续live_pending；提交态implementationCommit=null，本轮未提交或推送。随后仅进入Cursor，其他类别和最终跨类review仍未完成。


### 20.5 Cursor：R4-CUR-01 与长期资源父预算

**范围与来源。** 续用隔离树 `/data/projects/cc-switch-r4-codex-review.15ykc9zh`，以 Codex 收口后的独立增量基线开始；底层提交仍为 `e5b9e83ad488448473ad604fb855fbe28dd5f456`，不包含主仓并行 Share Market 的完整集成。已读取 Cursor/AGENTS.md 与 OmniRoute 根约定，按冻结 `443d66996d69d7ecf887fd61158404a3cca1b192` 读取 `cursor.ts`、`agentEndpoint.ts`、`builtinToolBridge.ts`、`cursorSessionManager.ts`，排除 21 个 tracked/1 个 untracked 变更。未发现新的专用 wire 缺口；动态 endpoint、工具约束、双 rail 和身份 fencing 继续使用已有实现。

**实际补强。** running/parked session 原已通过请求 reservation 接入父预算；本片为 completed Responses、API-key token/cooldown、模型/endpoint cache、discovery/exchange single-flight 和冷续接 claims 增加独立 RAII 归属。缓存和注册表使用有界 B-tree，索引按稀疏节点保守估算，避免删除条目后空 HashMap capacity 脱离账本。completed-cache 保持 10 分钟 TTL、64 MiB serialized 总量、8 MiB 单项、2000 项；序列化大小采用有上限的计数 writer，避免为判断能否缓存先分配完整 JSON 副本。其余原有 TTL、容量和精确身份条件不变。

- 新 single-flight 在发网前预留父预算；不够时返回本地 memory-capacity，避免未计量的并发 discovery/exchange。
- continuation claim 全批预留成功后才发布；父预算拒绝不消费半批 ID，使用既有 `cc_switch_request_memory_exhausted` 分类。条目数量耗尽仍保留独立 claim-capacity 错误，不把两者混淆。
- 同 key 替换若无法缓存，会移除旧 token、目录、endpoint 或 completed context；旧授权不会因容量压力复活。新获取的 token/目录/endpoint 仍可供当前调用者使用而不保留；completed context 缓存失败不改当前响应，后续缺失照既有 state-loss 合同处理。
- 关闭 session 时释放实际 Vec/String/HashMap capacity 和 opaque local-task state；先移除索引，再缩容 reservation。外部仍持有关闭后的 Arc 时保留外壳费用，直到最后 owner drop。parked 续接先向新请求预留，父预算失败仍保留原 owner 至关闭清理。
- 独立缓存和 claims 保持惰性过期；parked h2 继续使用主动 TTL timer。不能声称所有缓存到期时都已有后台 sweep，也不能把注册表删除等同于整个父账本归零。

**验证与反例。** 结构审查确认上述独立缓存原不接父预算，关闭的 `.clear()` 也保留了容器容量。首次 Cursor 定向运行 360 通过/1 失败：旧测试关闭会话后仍持 Arc，却断言预算为零，新账本正确留下 1,898 逻辑字节外壳。已改为先检查只留小外壳、再在最后 Arc 释放后归零；失败日志独立保留。h2 夹具首次严格编译另报异步请求体 `Unpin` 错误，固定 body 后严格全目标 Clippy 与 364 项 Cursor 定向测试通过；这轮包括 15 项新增 R4 测试。随后补充第 16 项转义 JSON 长度边界，随最终完整回归验收。

16 项 R4 合同覆盖 cache/request clone 双 owner、父预算拒绝、替换/计数淘汰/惰性过期、credential/目录/endpoint 归属、原子 claim、single-flight waiter、parked 转移失败和关闭外壳。纯回环 h2 假上游实际观察取消和首内容超时后上游 request 结束、response owner drop；OAuth CLI 与 API-key SDK 两 rail 的 parked TTL/容量淘汰均关闭旧流，新的 token 代际下迟到工具/response ID 不能获得旧 stream。夹具只组装本地传输和 registry，未访问厂商、未放宽生产 HTTPS/endpoint 校验，也不证明真实刷新/工具/图片接受性。

**计时、证据与回退。** 本类已有 `firstTokenMs` 从 native AgentService driver 入口计时，晚于共享 request preparation 与 managed-auth refresh；只有非终态批次的渐进业务输出才有首内容时间，header、KV/context/heartbeat 不满足 first-output deadline。h2 deadline 从发出请求开始，工具续接后重置绝对首输出阶段。本片记录该范围，未新增完整 milestone 或成功 socket-write 计量，也无 Cursor 的 RSS、p50/p95/p99、生产并发或提速结论。新的 R4 working-tree review 不修改八条旧 observation、旧 CUR-N2 no-wire 结论和两 rail receipt schema；`implementationCommit=null`，OAuth/API-key 各自 `live_pending`。父预算默认继续只观测，回退可重启并设 `CC_SWITCH_PROCESS_MEMORY_BUDGET_MB=0`，不撤销请求预算、身份栅栏、有界缓存或实际释放逻辑。

**最终离线检查。** 最终 `cargo fmt --check`、`cargo check`、严格全目标 Clippy、完整 Rust **3914 passed / 2 ignored** 与 `cargo build` 通过；其中 16 项 R4 新用例全部通过。冻结输入在各 Rust 步骤及后端审计期间均未变。后端 Node **1917/1917** 和另外 12 项源码/Provider/conformance/产品边界/Phase0/coverage/bridge/文档/静态 UI matrix/脱敏/差分检查通过。新构建的本地后端 smoke **10/10**，strip-debug 副本全部 ELF SHF_ALLOC 节保持一致，listener、临时 config 和 launcher 清理完成，日志为 0600 且不输出完整 token 日志。上述成绩的范围均为隔离树，不能声称涵盖主仓并行 Share Market 完整集成。

**主仓增量同步与离线 readiness。** 18 个源/文档文件按 Codex 收口基线生成增量，无重叠地应用到主仓 `075924cb3730eca0a300648a9d4c87e83c2c8dea`；876 个无关文件摘要不变，未新增无关文件。coverage 与 Phase0 三份生成件从主仓重新生成。主仓 `cargo fmt --check` 及 12 项相关源码/合同/文档/脱敏/差分检查全部通过，核验期间 Rust 源码不变；这不替代包含并行 Share Market 改动的主仓完整 Rust 回归。

本次 `RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 离线 readiness 实际退出 1、`not-ready / failed`：外部 Router 的两处退役 Market 端点、缺 Share Contract v8 reader constant、缺保留 Market 能力四处合同问题仍在，八项真实输入缺失，且保留 `local-contracts-unverified` 和 `deployment-not-tested`。未修改外仓或放宽门禁。Cursor 本类离线切片收口，两个 rail 仍 `live_pending`，`implementationCommit=null`；下一类为 Grok，八类总体与最终跨类 review 仍未完成。没有提交、推送、子代理、UI 自动化或部署测试。


### 20.6 Grok：R4-GR-01 与媒体/replay/目录资源归属

范围延续 Cursor 收口后的隔离增量基线，底层提交仍为 e5b9e83，未覆盖主仓并行 Share Market 完整集成。已按 Grok/AGENTS.md 核对推荐 grok2api@e2fd7ab4 和 sub2api@ab99d56e 的提交态：前者干净，后者六项未提交修改排除；CLI adapter/gateway、Web/SSO边界、凭据CAS、free quota与CLI身份七份文件的精确来源追加到r4Sources，旧历史不覆盖。

R4-GR-01决定 keep_current_retry_contract：参考在gateway逻辑请求外生成Idempotency-Key，TransientRetry跨selector尝试递增，5xx排除525/526。目标不存在该新5xx重提交合同，也不自动生成这两个header；回环实际观察500/503/525/526各一次POST，并确认不信任客户端自报计数。保留同绑定401、明确proof拒绝和WS→HTTP fallback的原有turn、attempt/elapsed与commit门槛。已有凭据CAS测试覆盖旧refresh success/failure晚到、bearer切换后迟到replay提交；不另造账号池。新header/版本接受性继续live_pending；Web consent/Imagine、质量自动重试/封号、95%本地估算额度均不迁入。

本片补父预算的长期owner：replay proof与CAS占位改B-tree，缓存miss可不安装generation，替换预留失败移除旧proof；请求clone和缓存分别计费。媒体归属采用不可变Arc表，候选在复制条目前预留，持久化/读取共享同表；容量或写盘失败保留旧表，最后Arc释放才退账。schema4、4096项与既有身份键不变。模型目录Arc payload、128项、revision/凭据门槛不变；无法发布新目录返回本地503而不刷新凭据。OIDC/JWKS各8项，预算拒绝只跳过缓存，当前文档继续按原验证处理。序列化大小改有上限计数writer，不为校验再分配完整副本。

复核发现媒体任务提交通用错误分支会将新本地容量错误记为Provider失败；已单独走memory-capacity日志/usage和capacity shed，回环验证无账号健康处罚且不重放。若上游已生成任务而本地归属提交失败，任务可能已存在；此时失败关闭，不声称跨上游/磁盘原子性。逻辑稀疏树/JWK估算不是RSS，控制面响应/解析/持久化scratch和一般账号/config状态不在全覆盖声明内。缓存仍惰性过期，未增加后台sweep。本类firstTokenMs各自起点已记录：HTTP晚于绑定/准入、早于adapter/auth refresh；WS从UsageTurn开始，晚于API鉴权和物理连接；媒体另从执行入口开始。没有新增全milestone、成功socket-write、Grok分位数/RSS或提速结论。

首轮生产cargo check通过，新增测试因误用未引入uuid而编译失败；改用现有opaque_id，未新增依赖。修复后严格全目标Clippy、Grok定向281/281通过，其中16项R4合同覆盖独立owner、cache miss/替换/淘汰/过期、Arc最后释放、候选准入/写盘失败、目录/JWKS压力、转义JSON和实际错误分类。两轮Rust调用均冻结输入，期间未变；失败记录保留。完整Rust、后端Node、本地smoke、主仓同步和readiness尚在收口，不能以定向结果冒充全量。三个真实operation仍live_pending，implementationCommit=null；最终跨类review未完成，无UI自动化、部署测试、提交或推送。


完整回归期间补审发现媒体快照的提交后错误对账会再次申请整表resident owner；old/live与candidate恰占满预算时，即使磁盘已提交也可能无法确认。已为通用持久化trait增加可覆盖的对账值读取入口，Grok特化只加载短期plain store/value，不制造第三份长期owner；其他存储保持默认语义。第17项R4回归模拟已写盘后报错，在两个owner耗尽预算时仍确认提交，且最后释放归零。原先所谓final一轮仅fmt/check及Node1917、12项静态检查完成，完整Rust仍在编译时主动中断（exit130），没有完整Rust/build/smoke结果；该轮报告与日志以pre-reconcile单独保存。以下最新全量验证须覆盖修复后的冻结输入。


Grok 修复后完整离线检查：fmt/check、严格全目标Clippy、cargo build通过；完整Rust为3931通过/2忽略，其中17项新增R4全部通过。后端Node1917/1917及另外12项相关源码/合同/文档检查通过，另核对五项状态写入/分层约束。全部385份冻结构建输入在各Rust阶段及后端检查期间不变。新构建本地后端smoke10/10，strip-debug副本全部SHF_ALLOC节一致，临时listener/config/launcher已清理，日志0600且未公开token日志。成绩范围是隔离树，不包含主仓并行Share Market完整集成。媒体提交后对账不申请第三份resident owner，容量错误不罚账号；review保留固定绑定、commit/CAS、可选缓存miss及未覆盖scratch边界。下一步只同步本类16文件增量并核验主仓，不将新实现冒充提交态或厂商真实接受。


Grok 主仓同步与收口：16个源/文档文件按Cursor收口基线增量应用，src/state.rs与并行主仓有差异但hunk预检/应用均无冲突，880个无关文件摘要不变、无新增无关文件，coverage和Phase0三份生成件从主仓重生。主仓fmt及12项相关源码/合同/文档检查通过，核验期间Rust源码不变；不替代并行Share Market的完整Rust集成。实际RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 readiness退出1/not-ready：外部Router两处退役Market端点、缺Share Contract v8 reader constant及缺retained Market能力四处问题仍在，八项真实输入、local-contracts-unverified和deployment-not-tested保持阻塞。未修改外仓或放宽门禁。Grok离线切片收口，三个operation仍live_pending，implementationCommit=null；下一类Kiro，随后CodeBuddy与最终整体review。无提交、推送、子代理、UI自动化或部署测试。


### 20.7 Kiro：R4-KI-01 与 Prompt Cache/writer 资源归属

推荐kiro.rs@9066e9c4/tree5d22f040工作树干净，七份提交态来源已追加，未执行外仓。contextUsage减输出继续shadow-only/live_pending，metadata/metrics/显式零的优先级与四auth kind×两region合同不变。现有CRC、16MiB frame、空参数恢复、身份scope、SQLite CAS和Compact零发网均保持。

本片已实现独立缓存/dirty owner、B-tree索引、pending与in-flight合计8192上限、原子发布前准入，以及无writer不积累无用dirty。失败批次移动owner回队列，不需再次预留，旧batch不能覆盖新generation；容量拒绝保留原TTL/state/删除记录，只停止本次可选估算，不制造命中、不罚账号、不改变vendor usage。expiry仍惰性，SQLite/legacy启动解析及连接scratch不在完整覆盖声明。逻辑估算不是RSS。

十三项R4回归已写入：七项Prompt Cache/写盘合同、四项目录/IDE owner合同及两项真实EventStream adapter owner合同；后者覆盖取消、EOF、截断和idle timeout，外部Bytes在stream drop后可继续持有owner，最后释放才退账。它们不证明厂商socket或取消健康分类。已有firstTokenMs/完整frame与idle deadline口径保持，没有新增milestone、socket-write或性能分位数。当前尚待编译/完整离线回归和主仓增量同步，不能以已写测试冒称通过。八路RUN_REAL=0均blocked_inputs/live_pending，无UI自动化、部署测试、提交或推送。


Kiro 资源清单复核补入独立模型目录与IDE版本缓存：原模型HashMap没有计数上限/统一惰性清理；现B-tree最多128目录，5分钟fresh/24小时最大stale不变，cache snapshot的Arc payload/owner共享直到最后释放。发布前预留，压力下移除被替代旧目录，当前已获取结果继续返回且不缓存；IDE仅一份可选保留值。四项新增回归核对最后owner、替换拒绝、淘汰/到期、IDE压力。控制面返回Vec/String投影及解析scratch未因此全覆盖。原有跨区域discovery候选只在既定条件下保留，不扩大推理的区域/认证rail恢复。第一轮fmt/check/严格Clippy通过后，在Rust定向测试编译阶段主动中断（exit130）补齐此遗漏，完整测试未获结果；旧报告保存，新输入须重新验证。

第二轮冻结387份构建输入后，fmt/check、严格全目标Clippy及Kiro定向243/243通过，十三项新增R4全部通过，输入在所有步骤间不变。review仅提升为working_tree_fixture_verified，仍不是提交态或厂商真实接受；implementationCommit=null，八路live_pending。接下来冻结最终资产，执行完整Rust、后端Node、相关审计及本地smoke，再按本类增量同步主仓。


Kiro 修复后完整离线检查：最终fmt/check、完整Rust3944通过/2忽略及cargo build通过，其中十三项R4全部通过；严格全目标Clippy引用第二轮相同321份Rust源码的通过记录，之后只更新review状态JSON，未改变Rust实现。387份最终冻结输入在Rust步骤与后端审计中不变。后端Node1917/1917及12项相关源码/合同/文档审计通过，另核对七项状态写入/依赖方向约束。本地后端smoke10/10，strip-debug副本全部SHF_ALLOC节一致，临时listener/config/launcher清理完成、日志0600。review核对dirty与in-flight owner移动、批量准入原子性、旧generation不能覆盖新状态、目录压力不复活旧授权及EventStream输出Bytes末次释放；计量优先级、CRC/截断和固定身份仍保持。逻辑保留量不等于RSS，不声称解析/SQLite内部scratch已全覆盖，也不证明厂商socket、八路真实接受或取消健康分类。结果范围是隔离树，未包含主仓并行Share Market完整集成；八路继续live_pending，implementationCommit=null，下一步按本类增量同步主仓。


Kiro 主仓同步与收口：13个源/文档文件按Grok收口基线增量应用，hunk预检及应用无冲突，885个无关文件摘要不变、无新增无关文件；coverage及Phase0三份生成件从主仓重生。主仓fmt及12项相关源码/合同/文档检查通过，核验期间Rust源码不变；不替代主仓并行Share Market完整Rust集成。实际RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 readiness退出1/not-ready：外部Router四处合同问题、八项真实输入缺失、local-contracts-unverified及deployment-not-tested仍在，未修改外仓或放宽门禁。Kiro离线切片收口，八路真实验收仍live_pending，contextUsage减输出仍shadow-only，implementationCommit=null。下一类CodeBuddy，之后做最终跨类review；无提交、推送、子代理、UI自动化或部署测试。


### 20.8 CodeBuddy：R4-CB-01 与目录/并发发现资源归属

只读研究cli2api@c7e4e73f/tree06520c39的客户端身份、refresh、response-head deadline和WAF相关实现/测试，共11份committed对象及明确函数锚点；排除未跟踪proxy.html/proxy.md，未执行外仓。本仓已有超时、非认证refresh分类、11128诊断、11148历史修复、终态usage和取消guard，不重复从零开发。参考CLI2.161.0、Global/CN Desktop5.8.4/5.8.3、IDE2.168.2及chat-only IDE/Product/Session组合只构成差分候选。没有本仓两站真实接受证据，保持现有2.142.0整套身份与目录来源，不单独升UA，不把Global desktop目录更多模型外推到生成。个人CLI X-Private-Data=false保持；企业不随此项开放。WAF正文重写不采纳。

本地目录改为最多128项B-tree，保留8分钟fresh/24小时最大stale，查询或发布惰性清理。不可变Arc payload与resident reservation共享到最后snapshot释放；同key新目录无法缓存时移除旧授权，当前新结果仍可用且不缓存。新single-flight发目录请求前准入父预算，最多128个活跃/保留注册；等待者与guard共享owner。容量拒绝经state/API/forwarder映射为本地503 memory-capacity，不刷新、不重放、不计Provider NetworkFailure。索引/旧snapshot保留量为逻辑估算，控制面fetch/parse及不缓存结果scratch不宣称全覆盖。

八项R4回归已写入：六项目录/flight owner合同、两站×三入口的目录准入拒绝零请求/无lease泄漏、提示词/思考/工具结果正文保真；尚待编译通过。现有两站三入口报文、固定账号401及取消/终态尾部usage夹具继续复用，未伪造新身份接受。firstTokenMs仍是现有尝试起点至第一个非空下游transform输出，header与body/idle deadline保留各自范围；没有新增milestone、socket-write或性能结论。两站RUN_REAL=0均blocked_inputs/live_pending，implementationCommit=null；完整验证及主仓同步待做，最终跨类review仍未完成。


CodeBuddy 首轮冻结388份构建输入，fmt/check、严格全目标Clippy及定向121/121通过，八项新增R4全部通过，输入在所有步骤间不变。先前“尚待编译”是当时状态；review现为working_tree_fixture_verified，实施提交仍为null，两站继续live_pending。随后冻结最终证据并完整验证，尚不宣布主仓完整集成或最终跨类review完成。


CodeBuddy 修复后完整离线检查：最终fmt/check、完整Rust3952通过/2忽略及cargo build通过，八项R4均通过；严格全目标Clippy引用首轮相同322份Rust源码的通过记录，之后只更新review状态JSON。388份最终冻结输入在Rust步骤与后端审计期间不变；后端Node1917/1917、12项相关源码/合同/文档审计和七项状态写入/依赖方向检查通过。新构建的本地后端smoke10/10，strip-debug副本全部SHF_ALLOC节一致，临时listener/config/launcher已清理，日志0600。review确认目录snapshot直到最后Arc释放才退账，替换拒绝不复活旧授权，single-flight waiter/guard保留owner，容量拒绝在两站三入口零出站且不刷新或罚账号；提示词、思考与工具正文保持。逻辑索引估算不代表RSS，控制面获取/解析及不缓存结果scratch不在全覆盖声明。成绩范围为隔离树，不含主仓并行Share Market完整集成；两站真实接受仍live_pending，保留2.142.0整套身份和目录来源，implementationCommit=null。下一步仅同步本类增量并核验主仓。


CodeBuddy 主仓同步与收口：14个源/文档文件按Kiro收口基线增量应用，hunk预检及应用无冲突，885个无关文件摘要不变、无新增无关文件；coverage及Phase0三份生成件从主仓重生。主仓fmt及12项相关源码/合同/文档检查通过，核验期间Rust源码不变；不替代主仓并行Share Market完整Rust集成。实际RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 readiness退出1/not-ready：Router四处合同问题、八项真实输入缺失、local-contracts-unverified及deployment-not-tested保持，未修改外仓或放宽门禁。CodeBuddy离线切片收口，Intl/CN两站仍live_pending，保留2.142.0身份及原目录来源，CB-N4保持禁用，implementationCommit=null。随后进行最终跨类review，包括早期独立缓存和Cursor取消分类的复核；无提交、推送、子代理、UI自动化或部署测试。


### 20.9 最终跨类 review：缓存所有权、取消分类与交付范围

本节复核八类 R4 切片，沿用 CodeBuddy 收口后的隔离基线。外部推荐仓仅作为既有提交态研究证据，本次 review 不执行或修改外仓，也不引入新账号池、版本头或能力开关。主仓并行 Share Market 改动继续按增量保护；完整后端成绩属于隔离树，不能替代主仓并行改动的完整集成。

**实际发现与修复。** 以下是代码中独立复核到的遗漏；25项新增Rust回归及完整离线验证已通过，具体成绩和范围见本节验证记录。

| 编号 | 实际缺口 | 修复与保持的合同 |
| --- | --- | --- |
| R4-REVIEW-AG-REPLAY | replay/CAS占位未计父预算，读取先深拷贝后申请请求预算，大小检查分配整份JSON | B-tree entry/tombstone owner；请求借用期间先准入再clone；未安装generation不能发布；替换拒绝不恢复旧proof；计数writer维持原编码/TTL/turn/item/entry上限，保留身份和CAS |
| R4-REVIEW-AG-CATALOG | 目录payload、key/index缺父owner，惰性清理只触及当前key | Arc共享payload/reservation，按身份/revision判定后准入并清理过期；拒绝返回当前未缓存目录、移除被替代授权；固定大小admission floor阻止较早in-flight结果在容量恢复后重装旧缓存，已有state发布revision栅栏不变。容量拒绝与superseded结果仍不同 |
| R4-REVIEW-QD-RUNTIME | session、目录、两类flight、response alias独立HashMap无父owner，session请求估算未含全部payload | 有界稀疏B-tree；session owner位于公开Arc的目标内，raw Arc末次释放才退账；目录不可变共享；flight在发网前准入，guard/waiter持owner。alias lookup与claim共用同一Arc，不深拷贝或提前退账；本地拒绝不刷新、不重试、不罚账号 |
| R4-REVIEW-CURSOR-CANCEL | 活跃流被下游drop仍记interrupted + Provider NetworkFailure | 记client_cancelled/Usage Interrupted，不发送Provider失败或成功样本；真实上游failed、429、parked完成分支独立保留。旧record_provider_outcome只写metrics，不能把此问题夸大成原来直接触发账号cooldown |
| R4-REVIEW-CL-ALIAS | Claude线程工具别名缓存及请求clone缺父owner，FIFO容器保留高水位容量 | 稀疏B-tree及固定sequence替代FIFO；entry owner与请求clone分别计量，clone前回调准入。相同别名重复发布不增长内存，冲突仍保留不可覆盖标记；128 aliases、1024 entries、64KiB/8MiB和10分钟TTL不扩大 |
| R4-REVIEW-EVID | Antigravity旧观察从当前工作树读取历史锚点，迁移准入后误报AG-OBS-0017缺失 | 22条历史观察从声明的本仓Git提交读取，新增baseline/implementation tree校验；当前capability/R4及八项资源回归锚点继续检查工作树。四项Node回归保证迁移可通过、提交树漂移/历史改写/当前准入缺失均拒绝，历史JSON和摘要不改写 |

Qoder新session payload准入失败返回本地503/cc_switch_request_memory_exhausted；已准入payload的可选缓存索引拒绝不影响当前session使用。目录缓存拒绝返回当前结果，绝不沿用旧目录。response alias是可选保留状态：内存不足时当前普通响应照常完成，下一次required continuation按既有严格state-miss处理；不把缓存失败冒充生成失败或消耗账号恢复预算。所有缓存仍为访问/发布时惰性清理，不宣称新增后台sweep。

**全部R4编号的处置。** 共享工具身份CORE-01、Claude CL01/02、Antigravity AG01/02、Codex CX01～04均已有实现或保持调用者约束的明确差分决定。CUR01、GR01、KI01、QD01、CB01分别按§20.2/5～8收口，保留各自fixed binding、身份/代际、提交后不重放、终态和用量owner。QD02已有独立CN PAT研究/拒绝复用Global rail的决定，证据不足继续不启用；KI01减输出仍shadow-only；GR01不新增无接受性证据的5xx重提交和header；CB01不单独升级UA或清洗提示词。EVID01的来源与历史追加策略不变；LIVE01的真实矩阵不从离线fixture提升。

**计时与可选优化的明确范围。** CORE-03已交付各类既有起点/deadline清单，以及Codex独立request/attempt/重试等待/语义内容/body handoff/WS本地writer完成/末次request owner释放和正式Server同负载测量。CX05本轮决定keep_current_encoding；没有厂商接受、端点/rail、codec scratch和flush证据，不开启新压缩或失败后自动明文重发。第19.13 G阶段的跨类统一新增milestone与其他Provider分位数测量保留为后续可选扩展，当前不标为已实现；本轮选择维持已有计时口径，不以统一字段名混合不同起点，不宣称八类统一首语义时钟、HTTP成功socket-write、提速或部署RSS硬上限。正式Codex资产是当时冻结源码的历史测量，不外推成这些review修复后的重测。

**预算覆盖边界与回退。** 父账本是逻辑保留字节/保守索引估值，默认观测；本轮覆盖上述八类已列举的活跃请求和专用跨请求对象。一般账号/config持久状态、控制面返回投影、获取/解析/SQLite/密码学/HTTP/TLS内部scratch和其他未列Provider缓存不因此全覆盖。Amazon Q的独立控制面目录与普通server状态不能以Kiro或八类的绿灯宣称已接父预算。设CC_SWITCH_PROCESS_MEMORY_BUDGET_MB=0并重启可关闭父硬准入，仍保留单请求sticky预算、有界缓存、身份栅栏与取消释放逻辑；不通过重置用量账本释放现有owner。

**审计修正的失败记录。** 第一轮后端Node1917/1917和其他相关合同通过，但Antigravity默认审计因历史AG-OBS-0017在当前facade查找已迁入缓存的snapshot.retained_bytes()而失败；其他七类默认审计通过。只读核对22条历史观察的21份不同提交态文件与全部tree均一致后，修正历史读取对象并保留当前实现检查。旧失败及第一次Node成绩另存，不把该轮写成整套通过；脚本修正不改变已冻结Rust构建输入，最终后端重新全量验证。

最终跨类 review 完整离线检查：fmt/check、严格全目标Clippy及cargo build通过；完整Rust为3977通过/2忽略、零失败，包含25项新增review回归。后端Node为1921/1921，其中四项新增审计回归通过；另27项相关检查（八类reference-delta默认与显式source审计、conformance、产品边界、Provider/Phase0/coverage、transform/bridge、文档、静态UI矩阵、脱敏和diff）及七项状态写入/依赖方向检查通过。Rust的391份冻结构建输入在各阶段不变，后端审计另冻结脚本输入。新构建本地后端smoke10/10；strip-debug副本全部SHF_ALLOC节一致，日志0600、临时listener/config/launcher已清理。完整后端成绩属于隔离树，未包含主仓并行Share Market的完整Rust集成；厂商真实receipt继续live_pending、implementationCommit=null，未做UI自动化或部署测试，也未提交或推送。

最终主仓同步与整体 review 收口：19个源/文档文件按CodeBuddy收口后的review基线增量应用，hunk预检/应用无冲突；884个无关文件摘要不变，无新增无关文件。src/state.rs保留主仓并行差异，coverage和Phase0三份生成件从主仓源重生。主仓fmt及另外27项相关合同/来源/文档检查通过，期间Rust源码不变；隔离完整Rust/Node/smoke成绩不替代主仓并行Share Market完整Rust集成。实际RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 readiness退出1/not-ready：只读Router合同仍报4处问题（frontend/components/i18n/locale-provider.tsx、src/market_fx.rs、src/models.rs、src/market_billing.rs），缺8项真实输入，local-contracts-unverified、deployment-not-tested保持；不以独立通过的后端检查改写该命令结果，不修改外仓或放宽门禁。八类离线实施及最终整体review收口，全部真实rail/operation仍live_pending，明确的可选扩展保留于上文；未提交、推送、调用子代理、做UI自动化或部署测试。

### 20.10 完成性复核与继续实施：CORE-03 跨类计时及主仓集成

上一目标轮次属于实际进展：完成五组资源/取消修复、历史审计校正、冻结后端验证与19文件主仓增量同步。§20.9 的成绩和边界保持，但其“全部离线实施收口”不足以证明原始目标的全部规划已经完成。本节以原始§19要求重新核验，不将已存在的较窄实现当作全部完成。

完成性复核起点源码显示 `request_timing.rs::TimingSlot::codex` 及 `forwarder.rs` 的普通HTTP计时准入只在 `oauth.openai_codex` 驱动启用；其他Provider的独立计时起点清单不等于一致request/attempt里程碑已接入。§19.12明确要求在确认语义后补齐统一里程碑，不能仅因§19.13 G包含可选增量，就把整个跨类计时交付移出本目标。继续按每次一类推进，先Claude，再依次补齐其余类别，保持原首token、成功、用量、恢复、deadline和固定绑定语义；HTTP只报告body handoff，不能冒充socket写成功。

主仓与上一隔离树另有并行Share Market差异。全量Rust/Node/smoke成绩目前只证明旧隔离快照；完成性复核还要对当前主仓的冻结快照做完整后端集成验证，并核对主仓源在验证期间是否变化。外仓继续只读，Router四处合同问题、真实输入缺失和禁止部署测试分别记录，不放宽readiness。真实回执缺失不能提升能力，也不阻止可独立完成的离线实现。

| 项目 | 当前证明 | 当前结论与下一步 |
| --- | --- | --- |
| 八类首轮正确性/资源切片 | §20.1～20.9；391份冻结输入、3977 Rust、1921 Node、smoke10及主仓增量验证 | 已有成绩保持原范围；继续按源码和真实测试结果核对，不重写历史观察 |
| CORE-03统一request/attempt | 全部八类及本轮声明的rail/协议入口已完成定向实现、验证和主仓增量同步，详见§20.10各类记录 | 已纳入当前主仓完整集成与最终review；各协议真实边界和独立门禁保持，见§20.11 |
| 当前主仓完整集成 | 当前主仓398份冻结输入；完整Rust4028通过/2忽略、严格全目标Clippy、Node1921、另28项检查和本地smoke10/10 | 离线集成完成；来源、限制和实际readiness结论见§20.11 |
| 真实接受与候选wire | 各类rail/operation保留私有receipt合同及live_pending | 不伪造接受；差分后的保守决定与需要真实回执的条件分别保留 |
| UI自动化/部署测试 | 用户明确禁止 | 持续不执行，不以缺这两类验证制造替代性完成声明 |

#### 20.10.1 Claude：统一观察器接入与取消边界

本类只接入 `ClaudeOAuth`，没有把普通 Anthropic API-key 或其他 Provider 提前计为已覆盖。`TimingSlot` 使用有限静态 Provider 标签，并冻结首次 request 的 Provider/surface；Messages、Chat、Responses、Gemini 三类 App Surface 的流/非流路径，以及独立 `count_tokens` operation，共用 request/attempt 里程碑。HTTP 起点仍是转发入口，不包含 API 收包和鉴权；各 attempt 保留独立起点，原 `first_token_ms`、deadline、提交、恢复、用量与健康规则保持。

SSE 复用 terminal detector 已解析的 JSON；非流式原生路径复用既有语义验证，转换路径复用既有 usage 解析结果，不为指标再建立完整响应树。语义 guard 关闭时的 `count_tokens` 仍单独做非权威观察，失败不增加响应拒绝；有效零计数也算结果。首语义内容只接受非空文本/thinking、有效 redacted data、带合法标识及对象 input 的工具开始或非空参数 delta；空 block、签名、ping、usage、停止原因与 block stop 均不算首内容。`max_tokens`、`pause_turn`、`model_context_window_exceeded` 只将本 attempt 的完成观察标记为 incomplete，不能继承到后续重试。

统一 forwarding future 的取消包装在销毁 pending future/预算 owner 前记录 cancelled，补齐等待响应头期间的取消顺序。流终态沿既有最终验证/结算路径观察；读流/转换错误只影响观察，不增加重试或 Provider 处罚。HTTP 只记 body handoff，未添加 socket 成功写入声明；最后已核算 request owner 的释放独立于语义终态和用量持久化。回环夹具覆盖九种成功请求形状、headers 等待取消、正文交付后的取消/上游错误及不重放、空控制事件、incomplete 与 attempt 隔离。

Claude CORE-03 最终定向验证：格式与include文件单独格式检查、cargo check、严格 `cargo clippy --lib --tests -- -D warnings` 通过；相关Rust测试574通过、零失败，包含本类新增5项、既有Codex HTTP/计时及共享adapter回归。393份冻结构建输入在这些步骤间保持不变，快照已包含冻结时主仓的并行Share Market改动；这次定向测试不替代最终全量集成。Claude/Codex reference-delta、coverage、Phase0、静态UI矩阵、文档索引、产品边界、transform、bridge与conformance共10项审计通过。生产review确认桥接JSON复用usage解析，Codex原地image-tool重试仅走Responses/Chat且标签一致，没有新增Gemini重试路径或成功socket写入声明。下一步仅增量同步本类10个源/文档文件，并核验主仓。

其他类别统一观察器、当前主仓完整后端集成与最终全部规划 review 继续待做；真实 operation 仍分别保持 `live_pending`，未提交或推送。

Claude CORE-03 主仓同步与本类收口：10个源/文档文件以冻结主仓为基线增量应用，预检与应用无冲突，897个无关文件摘要不变且无无关新增。主仓393份构建输入与本轮已验证树逐份摘要一致；主仓fmt、include单独格式、10项相关源码/合同/文档审计及diff检查全部通过，验证期间输入不变。本类574项相关Rust回归与严格lib/tests Clippy的范围保持，不冒充当前主仓最终全量Rust/Node/smoke集成。未改变registry/API或手改生成件。下一类仅推进Antigravity CORE-03；其余类别及最终全量集成/review仍待完成，总目标继续进行。全部真实operation/rail仍live_pending，未做UI自动化或部署测试，未提交、推送或调用子代理。

#### 20.10.2 Antigravity：两条 rail 的首内容与聚合路径

本类将统一观察器扩展到 `AntigravityOAuth` / `AgyOAuth`，保持两个独立静态 Provider 标签。按当前 registry/requirements，两条 rail 均覆盖 Claude 与 Gemini App 的流/非流，Gemini `countTokens` 使用独立 surface；没有把未登记的 Codex App 或 Gemini CLI 提前记作已覆盖。HTTP 起点、准入与 headers 沿共享转发入口，原 `first_token_ms`、first-byte/idle/overall deadline、用量、重试次数/条件和绑定保持。

非流式也会请求并聚合上游 SSE，因此在 `GeminiV1InternalSseAggregator` 复用已解析事件观察首内容，在成功 `finish` 后才观察聚合终态；流式复用 terminal detector 的既有解析，终态仍沿既有转换/最终结算路径。普通 JSON 和计数复用现有 embedded-error 检查的 Value，没有另建完整响应树。首语义内容识别实际文本/thinking、有效工具名及参数形状、媒体内容或 URI、可执行代码/执行输出；空文本、纯签名 carrier、usage、停止原因、心跳不算内容。有效 `totalTokens=0` 作为计数结果；负数/缺失不伪造首内容。`MAX_TOKENS` 只将当前 attempt 的完成观察为 incomplete，正常 blocked prompt 可有完整终态而没有首内容，不改原拒绝/成功规则。

两处 Google RPC 短延迟恢复的原有 sleep 分别记录 Retry wait，保留同账号、同 rail、原始延迟与总 attempt 预算；中间 attempt 不提前发布 request 终态。等待 headers、聚合尚未 EOF 或已交付正文时取消，都沿共享取消/owner 观察；HTTP body handoff 和最后已核算 request owner 释放独立，不声称 socket 写入成功。新增六项回环/语义用例核对两 rail × 两 App × 流非流、两 rail 零计数、聚合结束前首内容、headers/聚合/提交后取消、提交后 error 不重放、真实短延迟恢复与 attempt 隔离。

Antigravity CORE-03 最终定向验证：cargo fmt、include文件单独格式检查、cargo check和严格 `cargo clippy --all-targets -- -D warnings` 全部通过；相关Rust回归528通过、零失败，包含本类六项及既有Claude/Codex计时、Gemini/聚合/终态/adapter回归。394份冻结构建输入在这些步骤间保持不变，快照已包含冻结时主仓的并行Share Market修改。Antigravity/Claude/Codex reference-delta、coverage、Phase0、静态UI矩阵、文档索引、产品边界、transform、bridge与conformance共11项审计通过。最终生产差异review确认两处Retry wait包裹原sleep，聚合deadline、返回值、错误文本及原embedded-error判定保持；观察复用已有解析，不增加响应拒绝或重放。下一步仅增量同步本类8个源/文档文件并核验主仓。

当前主仓完整后端集成、剩余类别与最终全部规划 review 仍待完成；两 rail 及 Compact 等真实能力门禁保持 `live_pending` / disabled，未提交、推送、做 UI 自动化或部署测试。

Antigravity CORE-03 主仓同步与本类收口：8个源/文档文件以冻结主仓为基线增量应用，预检与应用无冲突，900个无关文件摘要不变且无无关新增。主仓394份构建输入与本轮已验证树逐份摘要一致；主仓fmt、include单独格式、11项相关源码/合同/文档审计及diff检查全部通过，验证期间输入不变。本类528项相关Rust回归与严格全目标Clippy的范围保持，不冒充当前主仓最终全量Rust/Node/smoke集成。未改变registry/API或手改生成件。下一类仅推进Cursor CORE-03；其余类别及最终全量集成/review仍待完成，总目标继续进行。全部真实operation/rail仍live_pending，未做UI自动化或部署测试，未提交、推送或调用子代理。

#### 20.10.3 Cursor：h2 流、缓冲重试与停放请求的计时归属

本类扩展 `CursorOAuth` / `CursorApiKey` 的有限静态标签，沿共享转发入口保留request起点和准入，四种下游格式覆盖登记的Claude/Codex/Gemini App。原 `firstTokenMs` 仍从native driver入口计时，且不把terminal-only/缓冲flush作为渐进首token；新增统一指标独立观察已解码并经过既有marker/工具处理的业务内容，不改变原first-output deadline、语义重试、usage或健康判断。

h2新建请求在原发送和headers边界观察，逐次实际body读取独立累计UpstreamRead wait；MCP/KV发送只确认入队，不声称上游socket写入完成。停放后续接转移原stream到新request预算，不伪造一组新headers；工具结果入队可以记录新request的SendStarted。工具输出完成当前turn，但parked h2及预算owner仍可能保留到续接、关闭或TTL。最后已核算owner释放沿真实预算析构记录，不能将HTTP完成写成整个会话资源已释放。

Cursor约束工具的流式驱动可能在Response已返回后于Body poll内部恢复/重试，因此共享HTTP观察器改为在poll之后读取当前TimingSlot，交付、带内错误及取消归到实际attempt；上一帧到下次poll的DownstreamDemand仍绑定上一帧的attempt。Auth/语义重试复用原固定surface和绑定，独立开始下一attempt并观察既有恢复准备等待；没有增加重试资格、次数、刷新或厂商请求。首内容与终态可以来自被丢弃的上游attempt，但中间attempt不提前发布request终态，且不把其内容冒充已交付结果。

新增本地h2用例覆盖两rail×四格式×流非流、控制/空内容排除、提交后取消并关闭双向h2、HTTP200中的带内失败、工具停放/续接预算转移，以及两rail约束工具的接受和预算耗尽；共享观察器另测Body内部切换attempt后的交付和取消。测试使用本仓回环夹具，不放宽生产TLS/endpoint校验或使用厂商凭据。结构缺口以Antigravity收口基线摘要及源码锚点记录。本类最终验证见下文；真实两rail继续 `live_pending`，回退可移除本类计时白名单/接线，保留全部资源、身份和协议合同。

验证过程的失败记录保留：首轮夹具误用 `InboundProtocol::OpenAiChatCompletions`，更正为本仓实际 `OpenAiChat` 后完成编译；随后相关回归666通过/1失败，失败用例在Rust部分移动后仍保留 `DriveOutcome::memory`，却提前要求最后owner释放。已明确绑定该独立响应owner：转移会话后仍持有它时必须未释放，显式丢弃后才要求释放；续接最终响应同样显式释放。此修正仅改新夹具，不改生产释放行为或降低断言强度，重新验证成绩另行记录。曾在格式化结束前捕获的初始快照亦被不变性校验拒绝，未进入编译/测试；正式成绩只采纳格式化结束后冻结且各阶段摘要不变的快照。

Cursor CORE-03 最终定向验证：cargo fmt、include文件单独格式检查、cargo check、严格 `cargo clippy --all-targets -- -D warnings` 全部通过；相关Rust回归667通过、零失败，含本类六项新回环/观察器用例及既有Claude/Antigravity/Codex计时、Cursor生命周期/身份/adapter/终态回归。395份构建输入在全部步骤间摘要不变；与666通过/1失败的上一快照相比，仅新夹具文件变化，生产源码不变。Cursor/Antigravity/Claude/Codex reference-delta、coverage、Phase0、静态UI矩阵、文档索引、产品边界、transform、bridge和conformance共12项审计通过。最终差异review确认工具约束、原重试条件/预算/绑定、TLS、deadline、usage和健康规则保持。下一步仅将本类10个源/文档文件增量同步主仓并核验；当前主仓完整后端集成及Grok/Kiro/Qoder/CodeBuddy的CORE-03继续待做。

Cursor CORE-03 主仓同步与本类收口：10个源/文档文件按Antigravity收口基线增量应用，预检与应用无冲突，899个无关文件摘要不变且无无关新增。主仓395份构建输入与本轮已验证树逐份摘要一致；主仓fmt、include单独格式、12项相关源码/合同/文档审计及diff共15项检查通过，验证期间输入不变。本类667项相关Rust回归与严格全目标Clippy的范围保持，不冒充当前主仓最终全量Rust/Node/smoke集成。共享Body观察器已随Claude/Antigravity/Codex定向回归核对，未改变registry/API或手改生成件。下一类仅推进Grok CORE-03；其余类别与最终全量集成/review仍待完成，总目标继续进行。真实rail/operation仍live_pending，未做UI自动化或部署测试，未提交、推送或调用子代理。

#### 20.10.4 Grok：HTTP、WS 与媒体的统一观察

本类复核确认：共享TimingSlot原先未准入GrokOAuth，WS新回合仅构造Codex时钟，图片/视频的独立转发路径没有统一request/attempt观察；完整Responses JSON直接识别为终态时还会漏记终态中已有内容。已将这些缺口的当前基线文件摘要与结构锚点保存于本轮审计记录，没有回滚生产源码制造失败。

按Grok/AGENTS.md只读核对grok2api `e2fd7ab481a30a1965bb61395d437154cf247c92` 的 `backend/internal/application/gateway/timing.go`、`backend/internal/infra/provider/cli/semantic_streamidle.go`、`backend/internal/infra/provider/cli/video.go`，以及sub2api `ab99d56e9626e6cd731592dae8553c9758a0efa2` 的 `backend/internal/service/openai_gateway_grok_sse_filter.go`、`backend/internal/service/grok_media.go`；五份文件均逐字节对应已提交对象。参考支持区分阶段等待、有效内容、ping及视频进度/结果，本仓观察器和取消/owner机制独立实现，不采用参考的超时、池化或重提交策略。

Grok HTTP接入有限静态Provider/surface标签，复用原发送/headers/body读等待；Responses JSON/SSE复用已有Value解析，首内容观察排除已审核ping、keepalive、doom-loop control和空delta，done-only终态内的文本、工具或不透明内容可以单独记录。观察结果不改变原Business提交、可见性、first_token_ms、usage、deadline、健康或重试合同；guard关闭时复用usage解析仍只观察，不新增拒绝。

WS每个response.create按真实Provider创建request/attempt，沿已有本地writer完成、读取、HTTP回退和资源所有权路径记时；同账号401刷新与原proof拒绝恢复分别保留retry wait/独立attempt。复用WS连接的新attempt不伪造新headers。HTTP回退仍受原pre-commit与总预算限制，提交后不重放。

图片/视频独立入口建立四个固定surface：image_generation、image_edit、video_generation、video_status。pending forward取消先于其future/预算owner释放；图片connected、空白、keepalive及纯progress不记首内容；JSON复用EOF已解析Value，SSE以借用RawValue和逐项数组观察避免再保留一棵大图/base64对象树，多行data仅在已有预算可容纳时做临时拼接。媒体的带内错误可记为计时failed，而原HTTP/JSON成功及能力/健康决策保持；EOF不覆盖已观察failure。视频提交在解析有效task id时记首内容；status请求完成不代表后台视频任务已完成。body handoff、终态和末owner释放继续分开，均不宣称HTTP成功socket写入。

新增本地回环覆盖三App/四推理Surface的JSON与SSE、流式EOF前首内容、headers/body取消、提交后错误不重放、媒体心跳/进度/图片结果、JSON编辑/视频提交和状态、401刷新及native WS/HTTP fallback；目前源码与夹具已写入，格式化、冻结编译验证、审计及主仓同步尚待完成。remote_compaction保持原有disabled/设计门禁，没有新增远程压缩执行路径；inference、media、remote_compaction真实验收仍分别live_pending。

Grok首轮冻结验证记录：397份输入在fmt/include格式、cargo check及测试编译期间保持不变；cargo check通过但提示MediaArray的Visitor impl嵌套位置，测试装配因新夹具直接修改Arc内plan而报E0596，未执行测试，Clippy未运行。已将impl移至模块作用域，夹具采用Arc::make_mut；没有把这次编译失败记为通过。

本类差异review又补强三处：媒体后续completed帧与EOF均不能覆盖已观察failure；完整Responses JSON的ErrorFrame通过独立document观察记failed，guard关闭也不新增响应拒绝；图片SSE的parser/多行拼接scratch使用请求及进程父预算的可选预留，拒绝只跳过观察，不能将转发预算标exhausted或改变恢复资格。正常必需预留仍维持原拒绝/耗尽行为。新增边界回归覆盖本地/父预算拒绝、共享scratch末owner、multiline CRLF、空白/转义/false错误形状和错误后的完成帧；后续须在重新格式化、冻结输入后验证。

Grok第二轮冻结验证如实记录：fmt/include格式与cargo check通过，397份构建输入全程不变，相关回归659通过、3失败，Clippy未运行。三处分别为新夹具喂入原normalizer拒绝的billing ping形状、误对Grok非流响应要求Codex专用SSE聚合、以及保留下游帧后的owner断言。前两项改为实际支持的ping/流式合同并保留非法形状拒绝断言，未放宽guard或启用Grok非流SSE聚合。第三项除将夹具明确读至hello业务帧外，review确认既有预留只跟随StreamForwardState；已将Grok普通输出及两种尾帧的原reservation移交实际Bytes owner，避免重复预留，且不延长account lease。

WS连接级观察槽仅持当前ForwardTiming，不持预算；每个turn、handshake/HTTP回退/proof恢复同步当前attempt。外层取消保护在pending future/owner析构前记录取消，异常返回补failed，正常断开仅结束尚未完成的turn，已完成turn保持结果。新增真实回环覆盖headers待定时abort、提交后abort和malformed上游失败不HTTP重放，另核对已完成turn与连接后续退出隔离。重新格式化、冻结验证和同步仍待完成。

Grok CORE-03 最终定向验证：fmt/include单独格式、cargo check、严格全目标Clippy全部通过；相关Rust回归664通过/零失败，另补跑全部名称含websocket的76项回归亦零失败，两组有重叠不相加。397份构建输入在全部步骤间保持不变；13项reference-delta/coverage/Phase0/静态UI矩阵/文档/产品边界/transform/bridge/conformance审计通过。前三项失败的修正与WS异常退出补强已由真实回环验证，未放宽ping形状、Grok非流SSE接受或提交后重放。Grok普通输出与两种尾帧转交原内存reservation给实际Bytes owner，不重复预留，不持account lease；共享WS槽不持预算且已完成turn不被后续错误改写。下一步仅按Cursor收口基线增量同步本类10个源/文档文件并核验主仓；当前主仓完整集成及Kiro/Qoder/CodeBuddy CORE-03仍待完成。厂商真实operation继续live_pending，remote compaction保持原门禁，未做UI自动化、部署测试、提交、推送或子代理调用。

Grok CORE-03 主仓同步与本类收口：10个源/文档文件按Cursor收口基线增量应用，预检与应用无冲突，901个无关文件摘要不变且无无关新增。主仓397份构建输入与本轮已验证树逐份摘要一致；主仓fmt、include单独格式、13项相关源码/合同/文档审计及diff共16项检查通过，验证期间输入不变。本类664项相关Rust回归与严格全目标Clippy的范围保持，不冒充当前主仓最终全量Rust/Node/smoke集成。共享WS/Responses观察器已随Codex等定向回归核对，未改变registry/API或手改生成件。下一类仅推进Kiro CORE-03；其余类别与最终全量集成/review仍待完成，总目标继续进行。真实rail/operation仍live_pending，未做UI自动化或部署测试，未提交、推送或调用子代理。

#### 20.10.5 Kiro：完整EventStream、内容与本地计数的观察

遵循Kiro/AGENTS.md，只读复核kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903的src/anthropic/stream.rs、src/kiro/parser/frame.rs和src/token.rs，三份工作树均与已提交Git对象逐字节一致。参考区分初始空块、实际内容、metadata与最终事件；本仓独立实现计时，不迁入参考的用量减法、state manager、超时或重试策略。当前基线明确缺Kiro/AmazonQ标签、独立send/401循环的headers/retry计时及builder内容观察，已记录摘要和源码锚点。

KiroOAuth和AmazonQOAuth沿共享入口保留request起点与准入，覆盖已声明Claude/Codex App的messages、chat/responses及本地count_tokens，不虚构Gemini Surface。独立HTTP循环观察send/headers与原401刷新准备等待，只在既有同账号刷新后开始新attempt，不增加请求、跨身份恢复、次数或期限。本地计数只记完成和body handoff，没有上游send/headers/首语义内容。

流式路径在既有SseBuilder构造Value时观察已规范化文本、thinking、redacted内容及已完成工具；初始message_start、空块、纯签名、usage、未知metadata和未完成工具参数不算FirstSemantic，不新增JSON解析。沿真实EventStream读取记录wait，完整endEvent只记语义终态，随后仍按原合同检查EOF、CRC、尾部帧与工具结束；异常和容量错误覆盖为failed，不能因先见endEvent提升成功。非流路径维持完整缓冲及既有严格解析，在成功解析的message Value观察内容和终态，不声称EOF前已经解析。旧first_token_ms、usage优先级及shadow-only减输出、健康、deadline、Prompt Cache和Compact门禁不变。

本地新增回环覆盖两产品×三Surface×流/非流及本地计数、控制/部分wire前缀排除、提交后截断/取消不重放、实际Bytes末owner、待headers取消和原401刷新attempt；builder回归另覆盖完整工具与thinking/redacted内容。Kiro四auth kind×两region仍独立live_pending；AmazonQ共享协议计时不代表其控制面目录或凭据已真实验收。格式化、冻结Rust验证、审计与主仓同步待完成。

Kiro第一轮冻结验证：398份构建输入不变，fmt/include格式、cargo check及14项相关审计通过；测试编译因新夹具未引入AUTHORIZATION常量报E0425，未执行测试、未运行Clippy。已改为与既有夹具一致的authorization头名查询，仅修测试命名，不改生产源码；重新格式化后冻结验证，不能把这次编译失败记为通过。

Kiro第二轮冻结验证：398份输入不变，fmt/include格式、cargo check及636项相关Rust回归通过；严格全目标Clippy因新刷新夹具的server句柄未使用而失败。已补显式server.abort()清理该回环服务，不改生产源码或断言；失败成绩保留，重新格式化退出后复验。

Kiro CORE-03 最终定向验证：fmt/include单独格式、cargo check及严格全目标Clippy全部通过；相关Rust回归636通过、零失败。398份构建输入在全部阶段保持不变；与第二轮相比仅刷新夹具增加显式服务清理，生产源码摘要不变。14项来源/合同/coverage/Phase0/静态UI矩阵/文档/产品边界/transform/bridge/conformance审计通过。本类七文件差异review确认原401恢复资格、次数与账号绑定、EventStream EOF/CRC/工具终态、usage/shadow、deadline和Prompt Cache门禁均保持。下一步仅按Grok收口基线增量同步主仓，Qoder/CodeBuddy及最终当前主仓完整集成继续待完成；真实八路及Compact等能力仍live_pending/disabled，未做UI自动化或部署测试，未提交、推送或调用子代理。

Kiro CORE-03 主仓同步与本类收口：7个源/文档文件按上一类收口基线增量应用，预检/应用无冲突；905个无关文件摘要不变，无无关新增。主仓398份构建输入逐份匹配已验证树；fmt/include单独格式、14项相关审计及diff共17项主仓检查全部通过，期间输入不变。本类636项定向Rust与严格全目标Clippy成绩保持原范围，当前主仓最终完整集成仍待完成。下一步仅推进Qoder；总目标继续进行。真实rail/operation继续live_pending，未做UI自动化、部署测试、提交、推送或子代理调用。

#### 20.10.6 Qoder：COSY读取、认证恢复与EOF终态

按Qoder/AGENTS.md推荐，只读研究TokenRouter当前提交d15137eb59d4d85bdc27ae563c37fcf9b0edebcd。已先加载project-doc、完整读取根索引和相关导航/章节，再按code-comments核对选定输出观察与执行注释；文件摘要见PROTOCOL_EVIDENCE。参考区分平台内容、sink写入、终态与断连，当前选定输出观察没有发现影响本切片的注释/实现矛盾；外仓文档影响仅作只读评估，没有修补、构建或执行外仓。既有“project-doc不可用”的历史记录属于当时会话，不冒充本轮门禁；本轮确已读取并应用仓内技能。

本轮基线缺Qoder静态标签及独立COSY循环观察，已记录基线摘要/锚点。QoderCosy沿共享转发入口保留request/准入起点；独立send/headers和每次真实body读取接等待计时；只有原先允许的同账号认证恢复才记录Retried/Retry等待并创建新attempt，OAuth刷新与PAT重交换不改变资格、次数、绑定或deadline。Responses续接仍禁止该恢复，不创建新推理策略。

解码器在原canonical Value处观察文本、reasoning及实际工具标识/名称/参数；role、空块、usage、空工具占位、finish-only和部分wire不记首内容，不重复解析JSON。length只作本attempt incomplete提示。原唯一终态加EOF成功后才观察终态，后续转换/身份/alias发布错误仍为failed。取消与末次实际Bytes/预算owner释放分开；不迁入参考的断连后后台收集或跨Provider故障转移，不更改用量、健康、COSY报文、站点、目录准入、恢复预算或历史保留规则。

新增四项夹具覆盖三rail×四Surface×流/非流、EOF前首内容与延后终态、原认证恢复独立attempt、待headers及提交后取消/截断/实际Bytes末owner。格式化、冻结Rust验证、审计和主仓同步待完成；Global OAuth/Global PAT/CN OAuth继续live_pending，CN PAT仍候选disabled，未做UI自动化或部署测试。

Qoder首轮冻结验证：398份输入不变，fmt/include格式、cargo check与15项审计通过；相关Rust660通过/1失败，Clippy未运行。失败的新等待headers夹具误把含query的QODER_GENERATION_PATH直接注册为Axum path，真实请求先返回404；已仅将夹具注册改为实际path部分，保持请求query、生产代码和取消断言不变，重新格式化退出后复验。

Qoder CORE-03 最终定向验证：fmt/include单独格式、cargo check及严格全目标Clippy全部通过；相关Rust回归661通过、零失败。398份冻结构建输入全程不变；与首轮660通过/1失败相比只修夹具path注册，生产源码不变，失败记录保留。15项来源/合同/coverage/Phase0/静态UI矩阵/文档/产品边界/transform/bridge/conformance审计通过。七文件最终差异review确认独立COSY读取/headers、三rail原认证恢复预算与绑定、唯一终态+EOF、Responses alias和真实Bytes末owner边界保持；计时无权改变成功、用量、健康或重试决策。下一步仅增量同步主仓；CodeBuddy与当前主仓完整集成/review仍待完成。三rail真实验收继续live_pending，CN PAT保持候选门禁，未做UI自动化、部署测试、提交、推送或子代理调用。

Qoder CORE-03 主仓同步与本类收口：7个源/文档文件按上一类收口基线增量应用，预检/应用无冲突；905个无关文件摘要不变，无无关新增。主仓398份构建输入逐份匹配已验证树；fmt/include单独格式、15项相关审计及diff共18项主仓检查全部通过，期间输入不变。本类661项定向Rust与严格全目标Clippy成绩保持原范围，当前主仓最终完整集成仍待完成。下一步仅推进CodeBuddy；总目标继续进行。真实rail/operation继续live_pending，未做UI自动化、部署测试、提交、推送或子代理调用。

#### 20.10.7 CodeBuddy：规范化Chat内容、两站与独立恢复

按CodeBuddy/AGENTS.md，只读核对cli2api@c7e4e73f64e661028a235e71306dba54ece48435的WorkBuddy ttfb.go、ttfb_test.go和sse.go，三文件工作树均逐字节等于Git对象，摘要见PROTOCOL_EVIDENCE。参考将响应头等待与body生命周期区分，清理空thinking/tool字段；不采纳它的30秒默认、跨账号failover、忽略解析错误或遇DONE即停止聚合。本仓严格DONE+EOF、错误锁存、固定绑定、license/limit语义和原恢复预算继续保持。

当前基线缺CodeBuddy静态标签、独立发送/恢复/读取观察，已记录摘要。CodeBuddyOAuth接入有限标签，保留共享request/准入起点；send/headers及实际body读取记录对应等待。仅原有runtime、generation和nonstream聚合三处获准的同账号恢复记Retried/Retry，并在成功准备后开始新attempt，没有新增恢复或发网路径。已有spawn/abort所有权保持，外层取消先记录，真实预算/Bytes末owner才记释放。

解码器复用既有sanitize后的Value观察文本、refusal、reasoning和实际工具字段；空字段/空工具占位、role、usage、finish-only及部分wire均不记首内容。length只提示当前attempt的incomplete，不改变原输出/用量；唯一DONE+EOF成功后才观察终态，后续转换/身份或内存错误保持failed。HTTP仍只报告body handoff，不声称socket成功写入；不改站点、客户端身份、提示词、目录准入、健康、恢复资格或deadline。

新增四项本地回环/解码夹具覆盖两站×四Surface×流非流、控制/部分wire排除与EOF终态、原401恢复独立attempt、headers取消及提交后取消/截断/实际Bytes末owner。格式化、冻结Rust验证、审计和主仓同步待做；两站真实receipt保持live_pending，不做UI自动化或部署测试。

CodeBuddy CORE-03 最终定向验证：fmt/include单独格式、cargo check及严格全目标Clippy全部通过；相关Rust回归665通过、零失败，含本类四项新增回环/解码测试。398份冻结构建输入全程不变；16项来源/合同/coverage/Phase0/静态UI矩阵/文档/产品边界/transform/bridge/conformance审计通过。七文件最终差异review确认只在原三处同账号恢复接入attempt/wait，保持既有恢复资格、次数、绑定、spawn/abort所有权、唯一DONE+EOF、错误锁存、usage与deadline；sanitize后复用Value，没有增加解析、发网或成功socket-write声明。本轮没有失败的Rust验证轮次。下一步仅增量同步主仓；随后对当前主仓做完整后端集成及全部R4最终review。Intl/CN真实验收继续live_pending，未做UI自动化、部署测试、提交、推送或子代理调用。

CodeBuddy CORE-03 主仓同步与本类收口：7个源/文档文件按上一类收口基线增量应用，预检/应用无冲突；905个无关文件摘要不变，无无关新增。主仓398份构建输入逐份匹配已验证树；fmt/include单独格式、16项相关审计及diff共19项主仓检查全部通过，期间输入不变。本类665项定向Rust与严格全目标Clippy成绩保持原范围，当前主仓最终完整集成仍待完成。下一步仅推进最终主仓集成与整体review；总目标继续进行。真实rail/operation继续live_pending，未做UI自动化、部署测试、提交、推送或子代理调用。

<a id="enhance-r4-final-review"></a>

### 20.11 当前主仓完整集成与最终 R4 review

本次直接验证 `/data/projects/cc-switch-server`，HEAD 为 `075924cb3730eca0a300648a9d4c87e83c2c8dea`，实现位于未提交工作树。冻结398份构建输入，其中332份Rust文件，包含冻结时已保留的并行Share Market修改；后端审计另冻结脚本，共484份输入。以下成绩属于当前主仓，不沿用§20.9隔离树的全量成绩。CodeBuddy收口至全部验证结束，仓库文件清单和摘要无变化，无额外新增或删除；最终只增量更新本计划和PROTOCOL_EVIDENCE.md。

**完整后端集成。** 所有Rust步骤顺序执行：include文件单独格式检查、cargo fmt --check均退出后才开始cargo check；随后完整cargo test、cargo build、严格cargo clippy --all-targets -- -D warnings。全部通过，完整Rust六个测试目标合计4028通过、2忽略、零失败。CARGO_INCREMENTAL=0、CARGO_BUILD_JOBS=4、RUST_MIN_STACK=67108864；所有步骤间输入摘要不变，没有在编译期间修改源码、fixture或构建输入。

仅运行scripts/audit和scripts/smoke中的后端Node测试，1921/1921通过、零跳过。另28项检查全部通过：八类reference-delta各自默认与显式--check-sources、conformance、产品边界、Server Provider/Phase0/coverage、transform/bridge、文档索引、静态UI矩阵、后端状态写入/持久化与依赖方向、脱敏及diff检查。静态UI矩阵是源码/合同审计，没有运行浏览器或UI自动化；后端状态检查保留仓库原规则，未修改门禁。

新构建的当前主仓二进制通过本地后端smoke10/10。构建依赖文件确认来源于当前主仓；为本地启动制作的strip-debug副本与原二进制全部SHF_ALLOC节逐份一致。临时loopback listener、配置和启动器均已清理，日志权限0600；没有真实厂商生成或部署测试。

**按§19逐项核对交付及取舍。** 下表是当前结论，前文各次“待验证”和失败记录保留其发生时点，不被本表改写。离线决定完成不等于真实接受或提交态证据完成。

| R4项目 | 最终离线交付 / 推荐决定 | 保持的边界与证据入口 |
| --- | --- | --- |
| CORE-01 | 活动工具身份冲突明确拒绝；历史身份独立、稳定分配，不授权selector或未来调用 | 共享bridge与三rail最终COSY回环，零发网拒绝；正文/参数不递归替换，见§20.2 |
| AG-01 / AG-02 | 目录版本/UA/generation贯穿key、发送、stale、发布和消费；补前置签名回填及显式工具ID分流 | 保留token/CAS、成功空目录和两rail隔离；无ID歧义及模型/Compact接受性不猜测，见§20.3/9 |
| CL-01 / CL-02 | thinking→reasoning明细归一且总tokens不变；已审核Opus5.5强制工具明确400；pause_turn维持incomplete | 不静默required→auto，不把Antigravity委托套成原生Claude合同，见§20.1/3 |
| CX-01 | 原生Responses保留phase；标准Chat/Messages明确有损边界和降级观察 | 不发明私有签名或把commentary冒充encrypted reasoning，见§20.4 |
| CX-02 | 被动quota按字段时间、精确账号/代际发布；队列有界且有父owner | 不改变token记录、权威quota查询或健康冷却，见§20.4 |
| CX-03 / CX-04 | 已审核metadata/control与业务分开；原生interrupt有mode、身份、真实ACK和超时状态；HTTP fallback明确unsupported | ACK不制造首内容或生成终态；create发送后不透明重放，不伪造HTTP取消成功，见§20.4 |
| CORE-02 | 请求及本轮八类专用长期对象接进程父预算，准入、转移、替换、淘汰和末owner释放有回归 | 默认观测、显式配置硬阈值；逻辑估值不等于RSS，通用状态/解析/TLS/SQLite等scratch未声称全覆盖，见§20.4～9 |
| CORE-03 | Codex及其余七类完成统一request/attempt、阶段等待、首语义内容、终态/取消与实际末owner观察 | 低基数标签；原first_token_ms、成功、usage、deadline与恢复规则保持；覆盖与逐类成绩见§20.10 |
| CUR-01 | 两rail的parked/完成缓存、h2及预算转移有生命周期回归；下游取消不再记Provider NetworkFailure | 工具turn结束不等于parked会话资源释放；真实两rail独立live_pending，见§20.5/9/10.3 |
| GR-01 | 核对turn/idempotency与身份后维持当前wire；补媒体/replay/目录资源归属及HTTP/WS/媒体观察 | 不新增无接受证据的5xx重提交或header；Web/SSO不能代替CLI/OAuth，remote_compaction保持门禁，见§20.6/10.4 |
| KI-01 | EventStream/缓存与资源回归通过；contextUsage减输出继续shadow-only | 权威tokenUsage优先、显式零/CRC/EOF保持；四auth kind×两region仍live_pending，AmazonQ计时不代表其控制面已验收，见§20.7/10.5 |
| QD-01 | 完整源声明与当前可调用子集分开，保留仍声明工具的已配对历史 | 已撤销/未知工具历史仍按原过滤及真实接受性门禁，不伪造活动声明，见§20.2/10.6 |
| QD-02 | CN PAT研究及独立合同缺口已记录，推荐暂不开放 | 不复用Global rail冒充CN支持，见§20.2 |
| CB-01 | 完整身份tuple差分后维持受审2.142.0及原目录来源；两站资源和规范化内容观察回归通过 | 不单独升级UA、不做WAF提示词清洗；个人账号private-data、license/limit、唯一DONE+EOF及原恢复保持，见§20.8/10.7 |
| CX-05 | 完成codec及正式Server测量后的推荐决定为keep_current_encoding | 不开启新出站/下游/WS压缩，不对任意400/415明文重发；历史基准不外推为本次源码重测，见§20.4 |
| EVID-01 | 实际采用的来源、文件摘要、反例及决定已追加；八类默认/source审计均通过 | 历史observation不覆盖；implementationCommit=null，不伪造已提交实现；source审计仅证明声明范围，见§20.1～10及PROTOCOL_EVIDENCE.md |
| LIVE-01 | 沿用独立rail/site/operation私有receipt合同，所有真实状态保留live_pending | 离线实现、真实接受、候选能力与提交态分别记录；无回执不提升能力，见§19.14 |

统一观察器的白名单为CodexOAuth、ClaudeOAuth、AntigravityOAuth/AgyOAuth、CursorOAuth/CursorApiKey、GrokOAuth、KiroOAuth/AmazonQOAuth、QoderCosy、CodeBuddyOAuth。它不代表普通API-key Provider或未声明App Surface全部覆盖。HTTP起点为转发入口，不含API收包/鉴权；HTTP只观测body handoff，成功socket写入没有实现或声明。WS仅在实际本地writer完成时记录写入；入队、保守Sent、终态、用量持久化与资源释放各有独立边界。各协议复用既有解析和规范化结果，不把心跳、metadata、空字段或纯停止原因当首内容。

性能结论继续限于§20.4实际冻结的Codex测量。此次跨类用例证明计时接线与所有权边界，没有重新测量八类p50/p95/p99或证明提速；进程硬阈值仍需按部署内存选择。结构拆分沿现有facade和私有模块进行，没有为缩短文件而重写Provider框架、账号池或数据库。

**实际readiness和独立门禁。** 完整本地检查后，以RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0、空EVIDENCE_FILE执行原scripts/release-readiness.sh，实际退出1、decision=not-ready。当前只读Router审计仍有四项：frontend/components/i18n/locale-provider.tsx、src/market_fx.rs含退役Market端点，src/models.rs缺Share Contract v8 reader constant，src/market_billing.rs缺保留的Market capability。未修改外仓或放宽审计。

实际缺失八项输入：CC_SWITCH_SERVER_TOKEN、SHARE_ID、CC_SWITCH_SHARE_URL、ROUTER_BASE_URL、ROUTER_API_TOKEN、CLAUDE_PROVIDER_TOKEN、CODEX_PROVIDER_TOKEN、GEMINI_PROVIDER_TOKEN。该次readiness自身保留local-contracts-unverified；用户禁止部署测试，故保留deployment-not-tested。独立通过的本地全量验证不能改写该命令的结果。候选CN PAT、Compact/remote_compaction、未经验证身份或模型能力继续按各自门禁保持disabled/conditional；所有真实rail/operation仍live_pending。

**最终review结论。** 本轮可独立完成的八类离线增强、统一计时、当前主仓完整后端集成及整体review已完成，在本轮复核的协议/资源边界内没有待修复的离线问题。六组§20.9实际review修正及各类失败历史均保留，不能把本次全绿解释成从未失败。真实回执、外部Router合同与提交态证据仍分别待齐备；本次未提交、推送、调用子代理、执行外仓代码、做UI自动化或部署测试。
