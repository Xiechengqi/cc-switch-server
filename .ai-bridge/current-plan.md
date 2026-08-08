# Complete verified cc-switch-server repair plan and final review

Updated: 2026-08-06T15:59:15.217Z
Workspace: /data/projects/cc-switch-server
Target agent: Codex (codex)

## Plan

目标：在 /data/projects/cc-switch-server 当前 main 工作树上，继续完成 server-pre-fix.md 经事实复核后确认的修复；所有产品/实现选择自动采用下述最优方案。不要提交、不要重置、不要覆盖或删除现有用户修改。当前已有约 42 个 modified files，是本轮修复的一部分；只做增量修改。最终做完整 review，运行 Rust/前端全量验证。没有真实 Router/Market/OAuth 输入时，只能声称 offline/local readiness，不得声称真实 E2E 通过。

必须遵循 AGENTS.md：
- Server 是独立产品，Claude/Codex/Gemini 均是一级客户端。
- 状态写入只经 state.rs domain methods。
- 锁顺序 config -> providers -> accounts -> usage -> shares -> ui_settings -> sessions -> oauth_logins。
- domain 不依赖 api/clients/proxy；proxy 不依赖 api/http/clients。
- 保持 Router/Market 当前协议：多 app Share 的 ACL、限额、过期、描述、subdomain、价格百分比必须全局一致；per-app map 只是兼容投影。

当前基线：
- cargo test --locked --quiet：1901 tests，3 failed：
  1) api::grok_catalog_provider_tests::auxiliary_inference_routes_reject_disabled_share_surfaces：测试构造了只有 surfaceEnabled 的不完整 Bundle metadata，被新 invariant 拒绝；
  2) shares::upsert_canonicalizes_app_settings_to_the_share_expiration：全局 expires_at 被空兼容投影错误覆盖/判冲突；
  3) state::reload_rejects_invalid_subscription_graph_without_swapping_live_state：reload 只记录引用图错误仍发布候选态。
- web npm run typecheck 已过；npm test 26 files/123 tests 已过（只有 act deprecated warning）。
- ChatGPT 已增量修改 invariants.rs 的冲突判断：token/parallel/expiration 只在 global 与 projected 都显式存在时比较。继续完成对应赋值逻辑。

阶段 1，先恢复后端绿线，必须完成：
1. reload 原子性：src/state.rs reload 在 repair_integrity 后必须调用 validate_subscription_reference_graph；任何仍无效的 subscription graph 返回带稳定错误码/上下文的 Err，并且绝不能交换任何 live store、mark_published、rotate key 或 reload tunnels。保留现有 test 并补必要原子性断言。
2. usage 持久化发布：
   - push_usage_log、push_health_usage_log_if_due、update_usage_log 当前无论 push_and_persist/update_log_and_persist 成败都发布 candidate，修掉。
   - journal append 是提交点。append 前失败不得发布；append 后 rollup/snapshot/compact 失败要从 UsageStore::load_or_default 重载并验证目标 request_id 的完整期望 UsageLog 已落盘，匹配才发布 reconciled disk state 并按已提交成功返回、记录 warn；不匹配返回原错误且 live state 不变。
   - append_usage_journal_record 在 flush 后 sync_data，确保所称 commit point 可持久化。
   - 补 fault-injection/不可写路径测试覆盖：pre-commit failure 不发布；post-commit reconciliation（若现有 storage fault hooks 可用）内存/重启态一致；update 同样。
3. Share global policy：
   - 全局 ACL/forSale/freeAccess/tokenLimit/parallelLimit/expiresAt/officialPricePercent 是权威策略；appSettings/accessByApp/priceByApp 是兼容投影且所有 app 必须一致。
   - 空字符串 expiration、负数 token/parallel 是“兼容字段未指定”，不得覆盖已有 global；用 input.field = input.field.or(projected)。
   - 两边显式值冲突必须 PolicyDivergent。
   - 保住并扩展 tests：空 projection 被 canonicalize 为全局 expiration；显式不同 expiration/limit 被拒。
4. disabled surface API test：不要削弱 Provider Bundle invariant。把测试 fixture 改为完整合法 Bundle surface metadata，或用明确的测试层只读内存注入隔离 route guard；优先完整合法 fixture。保留“disabled share surface 返回拒绝”的业务覆盖。
5. 跑 cargo fmt --check（必要时 cargo fmt）、cargo test --locked --quiet，必须 0 failed 后进入下一阶段。

阶段 2，后端契约/可靠性：
6. Provider Bundle 身份：
   - ordinary Provider 的 bundle_id 必须 None；只有完整显式 Bundle Surface 参与 Bundle 查询/修改/删除/聚合。
   - ordinary provider id 与 bundle management id 冲突时稳定返回 cc_switch_provider_management_id_conflict。
   - 所有 bundle 操作先验证完整 metadata/family/app/credential scope，再写入；测试 C1 fallback collision。
7. credential scope：
   - registry-driven CredentialSourceScope::{Bundle,Surface}；google_oauth 等 family bundle-scoped，custom_http surface-scoped。
   - 不再维护散落 credential family whitelist；测试新增 registry family 自动受控、custom_http 不错误共享 key。
8. seal_store 隔离：一个无关/disabled/legacy surface 的残缺 bundle metadata 不得让所有 ordinary provider 编辑失败。对显式 Bundle Surface 严格校验；legacy partial metadata 进入按资源隔离的 repair/quarantine/fail-closed，不得全局 poison。加回归测试。
9. bundle 字段作用域不得以 enabled_surfaces < 2 跳过。只要是显式 Bundle，就按 registry scope enforce，单个 enabled surface 也一样。加测试。
10. subscription/shares：
   - 空 bindings 也必须做 provider/account 引用检查和 fail-closed repair；
   - startup/reload 对每个坏 Share 独立 repair/quarantine，好的资源继续可用；repair 后引用图仍无效则 startup/reload fail before publish；
   - quarantine journal 增加有界保留（按条数与/或文件大小，采用简单确定性上限并测试），避免无限增长。
11. trust boundary：不要在 Server 伪造本地 email ACL。Router 已完成 API token 鉴权、user_can_invoke_share(email,share_id,app)、URL app 检测。Server 只接受 Router 侧已经认证/授权的调用上下文；增加 Server<->Router 契约测试/注释，证明 direct local endpoint 不冒充用户授权。若当前协议没有可验证的签名身份上下文，不扩张 wire contract，仅 fail closed 并记录为真实 E2E 前置条件。
12. 被吞错误：
   - src/api/router.rs spawn_share_upsert_sync 不得丢弃错误：记录结构化 error/code/share id，且调用方可观测时返回/保存失败状态；不要让 detached task 静默。
   - src/api/invoke/handlers.rs 删除 auth file 的失败不得静默；按调用语义返回错误或至少结构化 warn，并测试。
   - client.rs response.json 的 let _ 不是 bug，保留传播语义。
13. 不要为未证实项制造改动：CursorOAuth 在当前 managers 早返回下不可达；clone 次数无 profiling 不改；本轮不做大规模协议重写。
14. 对超大 state/dispatch/forwarder 只做低风险、行为等价、边界清楚的提取；优先把纯 validation/DTO mapping/helper 移到现有 domain 模块，严禁为了行数做高风险重构。清理真实 duplicate deps 仅在 cargo tree/lock 验证可安全统一时进行。

阶段 3，前端完整收口：
15. i18n：
   - 补齐新增 server/provider/share/security/auth UI 在 en/zh-CN/zh-TW/ja 的 key；测试所有 locale key parity。
   - 未知浏览器 zh 变体不得错误默认简中/繁中，采用明确 normalizeLocale 策略与测试。
   - 统一 server UI 的翻译入口；composeText 只用于确实需要插值的文本，减少双 i18n 分叉，不做无价值全量重写。
16. 主题/可访问性：
   - 修复 muted == foreground，对浅色/深色给可读层级；消除主要页面硬编码颜色，映射到语义 token。
   - terminal、4 个 skeleton、status 样式补 dark；不要机械修改不影响可读性的装饰细节。
   - 不全局隐藏 scrollbar；提供细而可见的样式。
   - 全局 :focus-visible 清晰 outline；FullScreenPanel 正确 dialog/aria-modal/label/focus 初始与关闭恢复；icon-only 控件有 aria-label。
   - 极小字号/圆角/橙 CTA 只按 WCAG 可读性和统一 design token 修，不做主观换肤。
   - 清理确认无引用的 dead components。
17. 未保存变更保护：
   - BundleEditor、ServerSecuritySettings/其他 settings form 都要 dirty guard；切换 provider/关闭 panel/导航/刷新前确认，保存成功清 dirty。使用统一 ConfirmDialog，不用 window.confirm。补交互测试。
18. 异步/错误：
   - 清掉 types/omo.ts、useOpencodeFormState.ts 的 empty catch；保留 fallback 但结构化/用户可见。
   - 主要 mutation 的 console-only error 改为 toast/inline error；不重复 toast 同一错误。
   - LocalEnvCheckSettings、SessionManagerPage 使用统一 clipboard helper，处理 permission failure。
   - suggestShareSlug 必须 catch/显示可恢复错误，不产生 unhandled rejection。
19. ConfirmDialog 防重复提交；pending 时 disable close/confirm，错误可见；现有相关测试保持。
20. API token/secret：
   - 默认 mask，reveal 明确；revoke 确认；clipboard failure；按钮 loading/debounce；不在 React state/local storage/cache 保留 plaintext password/token 超过必要生命周期。
21. 数据刷新/性能：
   - 不保留全局 staleTime:0 + refetchOnWindowFocus 对所有 query 的组合；按资源分类：实时状态短 stale，配置/registry 较长 stale，mutation 精确 invalidate。补关键 query 行为测试。
   - 大列表至少做分页/增量渲染；优先简单稳定的分页，不引入重型虚拟化依赖。
   - 不为 U13 “无 URL router”做高风险全站路由改造；本轮只保证 panel 状态可恢复/可导航，记录 URL 深链为后续架构项，除非现有路由设施已存在且改动很小。
   - toast duration 按 severity/可读性统一。
22. 移动端：保留页面标题可访问（可视觉压缩但不能完全隐藏语义）；修复布局冲突。已修 password width 不回退。
23. 跑 npm run typecheck、npm test；按 package scripts 再跑 lint/build（若存在）。消除新的 act warning，旧 warning 若源于测试库 API 可小改。

阶段 4，最终验证/review：
24. cargo fmt --check；cargo test --locked --quiet；前端 typecheck/test/lint/build；若耗时可分轮但最终都要跑。
25. show git diff/status；逐项审查：
   - 不引入 domain/api 分层违例；
   - 不违反锁顺序；
   - 所有 persistent mutation save-before-publish 或 commit-point reconciliation；
   - reload/startup validate-before-publish；
   - no secret/plaintext leakage；
   - Router/Market wire contract 不变；
   - 无静默 catch / unchecked detached errors（有明确合理注释者除外）；
   - 测试覆盖新增稳定错误码和回归边界。
26. 输出执行总结：修改文件、关键决策、测试命令与结果、仍需真实 Router/Market/OAuth 环境验证的项目、任何有事实依据但因高风险而延期的架构债务。不要把真实 E2E 标成通过。

继续直到计划完成或遇到不可绕过的真实权限/外部环境 blocker；不要向用户询问产品选择。

## Implementation contract

- Work from this plan in small, reviewable steps.
- Keep edits scoped to the requested task and existing project conventions.
- Run focused verification before handing work back.
- Update .ai-bridge/agent-status.md with files touched, checks run, results, blockers, and review notes.
- Save the final review diff to .ai-bridge/implementation-diff.patch when practical.
- Append notable execution events to .ai-bridge/execution-log.jsonl when the implementation agent supports logging.
