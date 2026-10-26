# 账号重置周期

状态：本期实施、整体review及离线功能验收已完成；完整发布门禁仍为 `not-ready`（见末节）。未部署，所有供应商真实验收均为 `live_pending`。

依据 `/tmp/cc-switch-server-account-reset-implementation-plan.md` 实施。本文描述实际实现、证据及保守边界，不把规划建议或上游假设当成已验证协议。线格式权威为 Router `PROTOCOL.md`。

| 交付状态 | 本轮结果 |
| --- | --- |
| implemented | 账号周期内核、双存储、已有证据的来源、Router/API和两端UI已实现并review；证据不足来源按计划保守延期，门槛见下文 |
| offline_verified | 本期功能回归和完整静态检查通过，53项证据映射已登记；不等于完整发布readiness通过 |
| live_pending | 真实供应商验收未执行，不作为本期离线交付的等待条件 |
| enabled | 本轮未部署、未修改生产启用状态，未自动迁移存量用户 |

## 冻结语义

- `accountReset` 必须使用已验证的同一订阅来源、共享主桶与 UTC 毫秒边界。凭据刷新不是周期重置，删除/重建同 accountId 的旧响应不得写回。
- 请求及 supplemental usage 按原请求开始时刻和原来源归属。延迟发现边界后回算，不能在发现时清零；终点可以未知。
- 精简使用账本与既有 usage 提交一起持久化，保留毫秒精度和人工基线所需修订，不随 32 天明细删除。历史无法证明则 `history_incomplete`，不按零计算。
- 人工基线绑定来源和 cycleId，保存使用水位；同周期修正时间保留目标语义，新周期不继承旧基线。
- JSON 和 SQLite 两条 authority 均需验收；不得为此功能强制全局迁移。周期事件和指针同属 ShareStore 持久化单元。
- Router 必须显式协商 v8 能力；普通六种周期保留 v7 兼容。动态快照不增加管理员配置修订，不能用历史 max 恢复旧周期用量。
- 公共来源仅接受通过白名单校验的 executed regular，`observed` 合法；公告时间不冒充逐账号执行时间。banked、scheduled、watch 不触发。
- 兑换先落盘 intent，复用 requestId；不确定结果不换新 ID 再消费。2xx/窗口数量不证明主长窗已重置。

普通六种周期不变，存量用户不自动迁移。新用户仅在 preview 返回 ready、完整历史、明确来源且 Router 已协商时默认采用本模式，否则沿用 lifetime。同一来源边界共享，但各 Share/用户的用量、人工基线、并发和授权到期独立；市场用户仍执行冻结合同，新市场报价不开放账号周期。

## 内核、身份与计量

实现入口为 `quota_cycle.rs`、`quota_cycle_source.rs`、`shares.rs`、`quota_ledger.rs` 和 `state/account_reset.rs`。

来源按已证明的 principal/workspace/region 或 Provider-owned canonical credential scope 隔离，对外仅随机不透明 sourceRef。access token 刷新不换周期。Account incarnation/identity generation 与 Provider credential generation 阻止旧响应提交。独立 quota credential slot 沿用 Provider 总 credential generation，不宣称另有独立 slot 计数器；任一非 keep credential patch 都提高总 generation。

streamIncarnation 区分恢复后的同步流；cycleSequence/cycleId 标识新周期；cycleRevision 标识同周期修正；usageWatermark 标识使用修订。新观测仅更新 freshness，不修改管理员 configRevision。事件与指针、回执同属 ShareStore 持久化单元。旧事件不回退当前周期；跨过其他已确认边界的修正进入 needsReconcile；加载时验证事件引用、修正链、回执和公共游标。

第一版不声明任何可离线无限外推的 recurring adapter。已知下一边界只推进一次，标 knownNaturalBoundary/stale，随后 end 为空；不会断网每周凭空重置或将自然月换算成三十天。

精简账本保存不可变来源/开始毫秒、稳定 metering ID、累计 Token revisions 和持久化水位。详情与精简 revision 沿用同次 usage commit；准入前先 durable pending，崩溃遗留的不明使用不能等于零。按 source/epoch/bucket/share/user 建 scope 索引，SQL 另有来源/毫秒索引；当前查询仍遍历目标用户历史 records，尚非时间树 O(log n) 查询。没有新增 Token 预留系统，在途仍可能超额。

人工目标 T 的生效值为 `max(0,T+U(currentWatermark)-U(watermarkAtEdit))`。修正边界后两次 U 都重算，晚补录仍计入。新周期不继承旧基线，切出/切回同周期不丢合法基线。导入不信任外部只读投影/baseline，只保留本地合法账本和本地已有 baseline。单用户/整 Share 批量提交检查 config CAS 和打开时冻结的 cycle/revision，任一失败整批不改，409 要求重新预览。

保留策略显式为 retainAllExactRevisions：保留全部精简 revisions，含已结束周期，不随32天详情淘汰，不自动聚合/删除。250,000 revisions 告警；1,000,000 停止新的有限 accountReset 准入，在途仍结算。阈值是代码常量，不是运行配置；preview ledgerHealth/两端编辑器展示告警。归档/分区需另行实施。

缺来源、只有秒级/分钟rollup的历史不按当前绑定倒填，不伪造毫秒。先建立 coverage，等可靠新边界后才完整。有限配额初始化/历史不足、持久化失败或容量受限返回503；真实耗尽沿用原超额码。已有完整 stale 周期继续按已确认边界计量，普通周期不受新容量阈值影响。

## 供应商能力与延期门槛

25个 ProviderType 和20个 typed Coding Plan Profile 均有分类 fixture。supported 不等于实际绑定已 ready，更不等于真实已验收。

| 来源 | 当前能力与主桶/拒绝规则 |
| --- | --- |
| Codex OAuth | 主动WHAM、HTTP/WS被动、公共公告、durable兑换编排；仅明确604800秒共享窗，短窗/review/additional不混用；合法绝对优先，缺失才相对秒回退 |
| Claude OAuth | 主动usage及同身份被动头；仅共享seven_day，不随five_hour/Opus/Sonnet/Fable重置 |
| Kiro / Amazon Q | 条件接入，principal/region及两个rail隔离；主AGENTIC_REQUEST订阅base，trial/bonus不触发；只有next reset不倒推月起点 |
| Cursor OAuth | 条件接入Dashboard的included plan真实billingCycleStart/End；导入snapshot不能自动跟随 |
| Copilot | 条件接入live internal-user有限premium allowance；沿用paid/free UTC日期解析；unlimited/缺reset/snapshot拒绝，不猜30天 |
| Kimi OAuth / API Key | 条件接入，两个rail分源；usage/seven_day归同一shared weekly，模型桶不触发 |
| GLM/Zhipu CN/Global | 条件接入；明确TOKENS_LIMIT/CREDIT_LIMIT和weekly unit/number；缺单位/多主桶拒绝，区域分源 |
| MiniMax CN/Global | 条件接入唯一general active weekly；相对单位毫秒；inactive/session-only不造周窗 |
| Volcengine | 条件接入AFPWeekly.ResetTime毫秒与Coding QuotaUsage weekly.ResetTimestamp秒；单位正反例，禁止最长/最晚启发式 |
| Grok OAuth | conditional未开放；billing/task/media字段不足以证明共享主桶，不借Console SSO rail结论 |
| Qoder | conditional未开放；expiresAt/nextResetAt/9999不独自证明个人/组织主桶，尚未新增status双源采集 |
| Antigravity / AGY / Gemini CLI | conditional未开放；当前返回multiple_quota_scopes，尚未新增summary RPC/模型范围绑定，不选任意模型边界 |
| CodeBuddy / Trae | conditional未开放；包到期/换序/频率恢复不触发，待证明稳定可续主桶 |
| Ollama Cloud / Cursor API Key / DeepSeek Account | unsupported；activity/快照/余额不证明周期，不构造虚假Account |
| 普通Claude/Claude Auth/Codex/Gemini/OpenRouter/Bedrock/Nvidia/DeepSeek API | unsupported；按量/API key不套OAuth周期；typed Coding Plan按Family另判 |
| Bailian / Xiaomi MiMo typed Plan | unsupported；现有unavailable adapter，不用模型可用性/余额猜周期 |

延期项有误触发负例和拒绝原因；重新开放需要同rail的脱敏主桶/稳定主体/时间单位证据。Qoder研究仍受本会话无法正式加载TokenRouter project-doc的约束，未读取其业务源码。没有把未完成的统一内核改名为live_pending。

Codex WS当前只接受WHAM风格明确周窗，不声称覆盖全部真实WS schema。兑换 `reset_windows[{scope:"shared",limit_window_seconds:604800,reset:true}]` 仅是保守预留明确回执形状，尚无真实协议接受性证据；现有仅windows_reset数量的回执为unknown，fixture不能证明厂商支持新字段。

## 公共公告、调度与兑换

客户端只访问固定HTTPS codex-resets.com status/history，无身份头、禁止跳转；返回URL/文本从不发网或解释执行。status仅探测变化，事件来自严格history。接受executed regular的observed或白名单x_post作者thsottiaux；banked/scheduled/watch/未知source/type不触发。effectiveAt使用announced_at、basis=publicAnnouncement，是本地公告跟随规则，不是逐账号证明；仅影响启用本模式的Codex共享来源。

默认status60秒/history300秒，每页100/每轮10页、1MiB、10秒；独立ETag、Date/Age证据、query-bound缓存、持久cursor/high watermark与两天overlap。429尊重Retry-After；错误指数退避约10% jitter，上限一小时。status变化不打断history退避；304/旧200不刷新账号新鲜度；不加随机参数绕缓存。坏页/落盘失败不发布游标，可重放；到轮次上限保留continuation。

调度按实际启用来源去重，包含非active显式Codex绑定和Kimi，复用singleflight；当前每轮串行，全局并发1，最多60秒重扫，已知边界可提前唤醒。解绑/停用后下轮不调度，已发请求由提交fence拒绝旧结果；未声称可中途取消全部底层HTTP。

手动/自动兑换共用durable operation，发网前保存同一operation/request ID，401重试不换ID。明确主长窗回执与事件同提交；仅2xx/数量、超时、断连、落盘失败/崩溃为unknown，不换ID重复消费。confirmed后刷新失败只重试只读查询。unknown需人工审计，未提供自动重试消费或reconcile解锁接口。没有记录且0%→0%、终点不变的站外重置不可保证发现，UI/本文均不作该承诺。

## Router、API和两端UI

Router reader v2..v8，市场最低v5不变。Server普通未协商payload保持v7，显式accountResetV1后才发v8字段。能力缓存绑定Router URL/installation、300秒过期；能力丢失不静默降级。

动态投影不纳入静态fingerprint/configRevision/generation。此版复用完整descriptor持久同步和ACK：Notify主动唤醒+30秒重放；不是独立dynamic envelope，也没有精确usageWatermark ACK。按stream→source/epoch→cycleSequence→cycleRevision/usageWatermark合并，合法校正可以下降，旧周期不靠max复活。新stream全量descriptor作为恢复握手，Share级retired stream落盘，删除/新增用户也不能复活旧流。

历史新增nullable started_at_ms/source/epoch/bucket；旧秒级行unknown，不乘1000冒充精确。账号quota只用Server权威，不重复相加Router本地历史。

Server invoke `preview_share_account_reset`、HTTP账号周期preview与控制 `/_ctl/preview_account_reset`、Router管理preview贯通。双端单用户/批量逐人预览、冻结cycle CAS、隐藏清理anchor、开放终点/时间依据/同步质量/容量告警、草稿与动态分离；市场合同只读。

| 错误码 | 行为 |
| --- | --- |
| ACCOUNT_RESET_UNSUPPORTED / INVALID_TOKEN_PERIOD_ANCHOR | 配置422，不自动改七日 |
| ACCOUNT_RESET_ROUTER_UPGRADE_REQUIRED | 配置409/运行503 |
| QUOTA_CYCLE_CONFLICT | 409+最新只读周期/previewRequired，Router普通设置也保留409 |
| ACCOUNT_RESET_NOT_READY / QUOTA_HISTORY_INCOMPLETE | 运行503，preview显示原因 |
| QUOTA_LEDGER_UNAVAILABLE / QUOTA_LEDGER_CAPACITY_LIMIT | 运行503，不放宽额度 |

## 存储、锁和恢复手册

JSON usage schema2兼容读schema1；journal仍版本1，归属随原record提交，snapshot含精简ledger。SQLite v1→v2追加独立migration/checksum，不改旧迁移；SQL已提交/旧marker幂等补齐，禁止重新导入旧JSON；高版本拒绝。replace详情不删精简账，shadow/activate/rollback export验证水位、metadata SHA256和规范化列。

quota跨域gate为 `workspace → share_quota_mutations → usage_persistence.gate → shares persistence gate`，存储锁仍按config→providers→accounts→usage→shares。外层reference/provider/lifecycle遵守各域顺序，不反向申请。backup/reload统一前缀 `backup_operations → reference_mutations → provider_commits → workspace → quota → config/accounts/usage/shares/ui persistence gates`，冻结writer/checkpoint后取一致截面，不能只复制未checkpoint主库。外部HTTP/等待/Router通知不持存储锁。

恢复覆盖前durable写独立account-reset-restore-fence.json，不进backup targets且manifest禁止覆盖。restore/startup/reload应用固定新stream、抬高coverage、intent/dispatched转unknown，设置redeemRecoveryRequired。恢复中断标quota degraded、清协商并尝试reload，不能沿用旧内存低用量放行。

运维边界：

1. 旧备份无法证明之后的真实消耗，有限quota保持history_incomplete直到可靠新边界，不恢复成0。
2. restore后暂停所有新用券；当前没有解除redeemRecoveryRequired的接口。不得删fence/operation/改JSON绕过，应另行审计并实现安全reconcile工具。
3. schema2/accountReset写入后不能直接换旧binary。首选能读新schema、关闭新入口的兼容版；回迁旧schema需另设计停服离线迁移，不删表/ledger或擅改sevenDays。
4. 未新增独立公共采集/周期推进运行开关。紧急操作可暂停相关Share，不能关记账；灰度入口开关/归档/reconcile工具为后续运维增量。

## 53项离线验收映射

以下是确定性fixture/存储/静态UI证据，不是生产验收。测试名可在对应源码搜索；“保守拒绝”属于本期预期结果。

| 编号 | 测试/证据入口 |
| --- | --- |
| C01 | codex_selects_explicit_weekly_independently_of_position_and_uses_relative_seconds；review/短窗负例 |
| C02 | token_period的calendar_windows_use_utc_boundaries、fixed_windows_are_anchored_and_half_open；Cursor原窗口 |
| C03 | quota_delayed_reset_baseline_correction_and_mode_roundtrip_preserve_real_usage |
| C04 | millisecond_boundary_and_late_completion_are_exact；exact_revisions_survive_detail_replacement_and_rollback |
| C05 | 同C04及HTTP/Cursor/WS开始时刻贯穿，晚结算不直接加当前周期 |
| C06 | quota_revision_permutations_and_supplemental_identity_are_exact；revisions_deduplicate_and_baseline_watermark_remains_rebuildable |
| C07 | boundary_correction_keeps_cycle_and_increments_revision；freshness不换cycle |
| C08 | delayed_event_is_idempotent_and_old_event_does_not_roll_back；quota_repeated_quarantined_correction_does_not_grow_history |
| C09 | unknown_end_is_open_and_does_not_repeat_resets_offline |
| C10 | 当前无recurring adapter，不开放连续外推；C09锁定保守行为，普通calendarMonth/ThirtyDays独立回归 |
| C11 | 精简账详情裁剪/rollback、容量阈值不淘汰结算测试；retainAllExactRevisions |
| C12 | missing_history_or_different_identity_never_becomes_free_quota；秒级Router历史不进权威账 |
| C13 | C03及baseline watermark/revision测试；修正后两次U重算 |
| C14 | quota_old_cycle_and_mixed_batch_fail_atomically_and_reset_preserves_pause_and_total；409映射 |
| C15 | C03切出切回；Server normalize/export roundtrip，导入仅本地baseline |
| C16 | identity_fence_rejects_deleted_and_recreated_account_results；region/principal与既有workspace rebind回归 |
| C17 | quota_principals_are_separated_by_region_and_missing_identity_is_not_guessed；coding_plan_quota_waiter_revalidates_provider_generation_after_flight |
| C18 | C03多用户隔离；原Share并发/用户过期回归 |
| C19 | C14的pause/总预算，原认证/桶级cooldown fence回归，无新增解除路径 |
| C20 | revision排列、原Share in-flight并发回归；没有新增精确Token预留 |
| S01 | quota_shared_aliases_and_absolute_relative_bounds_are_strict |
| S02 | quota_unsupported_package_model_and_billing_fields_never_trigger；原Claude共享/模型桶回归 |
| S03 | 同上，Grok宽泛billing字段拒绝；主桶延期 |
| S04 | quota_kiro_q_base_and_cursor_included_windows_do_not_follow_packages_or_snapshots；region隔离 |
| S05 | 上述Cursor及quota_copilot_uses_only_finite_live_premium_allowance_and_real_reset，原paid/free日期fixture |
| S06 | quota_shared_aliases...；quota_coding_main_buckets_reject_guesses_and_keep_explicit_units |
| S07 | quota_coding_main_buckets...：MiniMax inactive/毫秒、Volc双schema单位正反例 |
| S08 | unsupported_package_model_and_billing负例：Qoder/CodeBuddy/Trae拒绝 |
| S09 | 同上及25 Provider分类；不开放跨组，summary采集延期 |
| S10 | 同上及20 typed Profile分类；不构造虚假Account |
| S11 | public page ingest+apply_reset+C03重算，事件不依赖利用率下降 |
| S12 | strict_executed_schema_does_not_interpret_text_urls_or_forecasts；banked不入事件 |
| S13 | public_quota_catchup_resumes_after_restart_and_bad_page_keeps_cursor；mock提交失败重放 |
| S14 | public_quota_history_etag_pagination_and_status_are_independent；status退避与HTTP429/304/旧200/redirect/body限制 |
| S15 | quota_redeem_unknown_confirmed_and_duplicate_receipts_are_durable；预留明确主桶回执正反例 |
| S16 | 同上、原401同requestId回归、restore fence阻止重消费 |
| S17 | referenced扫描/去重与原singleflight回归；非active显式绑定/Kimi被纳入，停用后下轮取消 |
| S18 | 同边界新观测只更新freshness；UI/本文说明不可观测重置 |
| R01 | usage journal replay/compaction、精简revision/跨详情与JSON重启 |
| R02 | quota_schema_marker_crash_recovers_each_authority_without_reimport |
| R03 | schema v1→v2幂等、高版本/旧checksum篡改拒绝 |
| R04 | exact_revisions_survive_detail_replacement_and_rollback；activate/load/export |
| R05 | failed_compaction_keeps_live_state_and_journal_authoritative；pending crash/metadata/中间revision篡改/mock持久失败 |
| R06 | 同单元提交失败不发布、原usage persistence reconcile、准入按最新水位重算 |
| R07 | quota_backup_and_reload_share_one_gate_order；quota_restore_fence_survives_old_backup_replay_and_is_idempotent |
| R08 | Router share_contract_v8_keeps_v6_v7_compatible_and_rejects_future_versions；v8字段/显式能力门禁 |
| R09 | Router quota_projection_orders_cycles_watermarks_and_accepts_downward_correction |
| R10 | 同上；quota_source_handoff_requires_config_revision_and_null_view_stays_pending |
| R11 | 动态fingerprint不dirty、descriptor去重/重复全量/恢复握手；复用ACK而非精确quota ACK |
| R12 | 0088 nullable精度迁移；账号不从本地历史推导，普通quota回归 |
| R13 | 双端roundtrip/冻结cycle/域批量原子CAS；quota_control_conflict_keeps_409_and_only_recoverable_fields |
| R14 | 双端动态quota不dirty、市场只读和六种普通周期回归 |
| R15 | fixed origin无auth、10秒/1MiB/100条/10页、非法cursor/循环/未来事件/URL不执行 |

证据分层为纯域/来源fixture、JSON/SQLite恢复、fake HTTP采集、Router内存SQLite同步、静态UI测试。没有浏览器自动化或真实厂商操作；完整双进程长时间断网混沌演练未作为已完成证据，关键旧流/丢包重放/回退用确定性测试覆盖。

## 整体review与验证

已修复：freshness动态修订、重复隔离修正膨胀、旧流跨用户复活、未接受descriptor提前发布stream、401后被动观测缺失、恢复中断旧内存、backup/reload锁反序、导入信外部baseline、批量冻结cycle被后台覆盖、Router丢周期409、旧版本/表数测试断言。运行时503保留各自稳定错误码，不再泛化为无可用供应商；公共采集使用统一直连客户端，不继承系统代理。Clippy发现的8项静态检查问题已修复并通过复跑。完整静态检查另发现Phase 0审计清单的3条源码SHA256过期，已通过原生成器更新并审查diff；未变更断言或兼容策略，相关6项单测复跑通过。

2026-10-26离线验证记录：

| 范围 / 命令 | 实际结果 |
| --- | --- |
| Server `cargo test --quiet`（RUST_MIN_STACK=67108864） | 3,685库测试、124 API测试、2项集成测试通过，0失败；另1项ignored未计为通过。已覆盖最终直连客户端改动 |
| Router `cargo test` | 1,503通过、0失败；后续仅文档变更，未重复耗时全量运行 |
| 两仓 `cargo check` / 格式检查 | 通过；Router已有dead_code警告不属于本期变更 |
| Server `cargo clippy --all-targets -- -D warnings` | 通过，无新增静态告警 |
| Server完整 `scripts/static-checks.sh` | 退出0；1,896项审计单测、13项smoke脚本单测、全部后续契约/边界/前端/文档检查通过 |
| Server Web typecheck / Vitest | 通过；41文件230测试通过 |
| Router Web typecheck / share-settings / share-email-usage | 通过；13+3测试通过 |
| Server Web `WEB_DIST_DIR=临时目录 npm --prefix web-src run build` | 通过；存在Vite大chunk提示，无构建错误；未覆盖已跟踪产物 |
| Provider coverage / UI matrix / runtime contract | 通过；25 ProviderType、82 Profiles，244 commands |
| docs index / 产品和Web边界 / SQLite契约 / i18n | 通过；文档49文件142链接，i18n无missing |
| 临时目录 `scripts/smoke/smoke-local.sh` | 退出0；本地health/version、初始化、登录、dummy Provider和普通Share通过，无真实上游 |
| 两仓 `git diff --check` | 通过 |
| 离线 `scripts/release-readiness.sh` | 退出1、not-ready，原因见下节；不以测试单独通过覆盖发布结论 |

完整静态日志保存在下述临时证据目录的`static-checks.log`。脚本内的real-harness测试为隔离fixture/mock服务回归，不是供应商真实验收。

已知保守边界为上述供应商门槛、无无限recurrence、unknown回执、恢复reconcile工具、精确quota ACK、长期归档及大规模性能优化。本期不部署、不读真实账号、不消费真实券；提交推送按用户另行授权执行。不得将条件/不支持供应商或尚未运行的检查标为真实通过。

## 发布门禁与交付边界

离线release-readiness已经执行，退出码1、decision=not-ready，不能写成发布通过。`RUN_TESTS=0`只表示该脚本没有重复执行测试；本期功能测试结果另行记录。真实输入缺失和未部署也分别保留，不归并为功能实现失败或虚构真实通过。

跨仓库Token Market审计仍有3处当前Router已提交基线与旧规则不一致：

- `frontend/components/i18n/locale-provider.tsx`的`/v1/market/config`被旧规则判为已下线接口。
- `src/market_fx.rs`保留同一配置端点，同样被旧规则命中。
- 审计仍要求`src/market_billing.rs`，而Router当前HEAD已删除该文件。

以上均已核对Router HEAD，不是本期账号周期改动引入；没有回退市场改动、放宽这3条规则或将它们改名为live_pending。账号周期所需的审计版本断言已同步为Server普通v7/显式v8扩展、Router v8 reader。市场审计基线需在独立任务中对齐，完整发布门禁通过前不声称release-ready。

本地证据目录为`/tmp/cc-switch-account-reset-verify.bQ9Mx0`，包含隔离smoke输出、offline-readiness.json、static-checks.log和Server前端构建。目录不含真实供应商输入；smoke日志含仅用于该临时目录的dummy登录令牌，不作为公开附件或提交材料。Server发布流程会从web-src重新构建资源，本轮没有覆盖已跟踪web-dist；Router未执行完整Next构建、浏览器或部署验收。
