# UI Parity 实施计划（对照截图逐区对齐 desktop）

基线：desktop 截图 `/tmp/95ba11101c2b17ef5719b1c968205e40.png` vs server 截图 `/tmp/b8fc8b66517b4820a33a926c74523592.png`（2026-07-04）。
本文件承接 `DESKTOP_ALIGNMENT_TASKS.md` Phase L/M/N，是 UI 对齐的专项任务清单；完成项同步回主清单。

## 一、为什么差距这么大（根因）

| # | 根因 | 说明 |
| --- | --- | --- |
| R1 | **Phase L 把"parity"做成了"功能覆盖"而非"界面对齐"** | 6 个 `*Dashboard.tsx`（7.6k 行）是对着 server API 手写的管理后台，信息架构自创：左侧边栏导航、页面大标题、统计卡片行、供应商类型矩阵网格——这些 desktop 一个都没有 |
| R2 | **没有复用 desktop 前端，而 desktop 前端本就能跑 web runtime** | desktop `src/App.tsx` 内建 `webRuntimeContext`（client-login / share-scoped 分支），配合 L4 已完成的 90 命令 invoke registry，desktop 组件可以直接移植；Phase L 却选择了从零重写 |
| R3 | **组件体系与设计令牌不同源** | desktop 用 shadcn/Radix + `BrandIcons`/`ProviderIcon` + 自有间距/圆角/配色 token；`web-src` 是手写 Tailwind，无一复用，视觉必然不同 |
| R4 | **内部诊断信息被提升为主界面** | server 把"可创建类型 17 / 仅诊断 2 / 可用供应商类型"矩阵放在主页；desktop 把类型/preset 收在"添加供应商"对话框里，主页只有干净的 provider 卡片列表 |
| R5 | **i18n 只接了标题级**（N2） | 默认 `zh` 下中英混排（"Share"、"authenticated"、类型卡英文副标题） |

## 二、未对齐点清单（逐区域，截图 + 源码核对）

### 布局壳
1. server 有左侧边栏导航；desktop 无侧边栏（顶栏 + 单内容区 + 对话框/全屏面板）。
2. server 有页面大标题区（"供应商 + email"）；desktop 无。
3. server 左下角暴露 `authenticated`/`test001` 调试态；desktop 无对应元素。

### 顶栏（desktop `App.tsx:1779` header）
4. logo：desktop 为蓝色 "CC Switch" 文字 + 设置齿轮按钮（打开 SettingsPage 指定 tab）；server 为侧边栏 logo 块。
5. `ProxyToggle`（绿色代理/share 开关）与 `FailoverToggle`（自动故障转移开关）在 server 完全缺失（对应 API 已有：failover/config）。
6. App 切换：desktop 顶栏 segmented pills（品牌图标 + Claude Code/Codex/Gemini）；server 是内容区普通 tabs。
7. 顶栏工具图标组（History、FolderArchive/备份、BarChart/用量、Download/导入、UpdateBadge）缺失——server 的对应功能都藏在侧边栏页面里。
8. 添加按钮：desktop 橙色圆形 "+"；server 蓝色文字按钮"添加供应商"。

### Provider 主视图（desktop `ProviderList`/`ProviderCard`）
9. 统计卡片行（供应商 0/可创建类型 17/仅诊断 2/健康 0）desktop 不存在 → 移除。
10. "可用供应商类型"矩阵网格泄漏在主页 → 收进添加对话框（preset/类型选择步骤）。
11. 卡片样式未对齐：desktop 有拖拽手柄（⋮⋮）、当前 provider 选中高亮（蓝色边+底色）、URL 链接行、右上"15 分钟前"+刷新图标、订阅徽章（Pro）+ 账号 email 行内展示。
12. 拖拽排序缺失（desktop `update_providers_sort_order`，registry 已有对应 shim）。
13. 空态样式/文案不同（desktop 有专门 `ProviderEmptyState`）。

### 对话框
14. `AddProviderDialog`/`EditProviderDialog` + per-app 表单（`ClaudeFormFields`/`CodexFormFields`/`GeminiFormFields` + OAuth Sections）+ preset 选择器（含 partner preset 卡片）未移植——server 是自制表单页。
15. 通用组件（`ConfirmDialog`、`JsonEditor`、`IconPicker`、`ProviderIcon`/`BrandIcons`）未移植。

### 设置（desktop `settings/SettingsPage` tabs）
16. desktop 设置是全屏面板 + tab（General/Language/Theme/Directory/Proxy/ImportExport/Backup/AuthCenter/About…）；server 是侧边栏单页堆叠。server-only 项（Router/tunnel/upstream proxy）应作为新增 tab 融入同一结构。
17. 主题切换 UI 缺失（Tailwind darkMode 已配置但无入口；desktop 有 `ThemeSettings`）。

### 其余 retained 页面
18. Share：desktop `SharePage` 组件族（ShareCard/ShareStatsBar/ShareToolbar/ShareRequestLogTable/TunnelConfigPanel/Owner 对话框）vs server Phase L 过渡实现。
19. Usage：desktop 用量视图（BarChart 入口、图表化展示）vs server 文本表 Dashboard。
20. Universal：desktop `UniversalProviderPanel` vs server Phase L 过渡实现。
21. Accounts：desktop 将账号/OAuth 集中在 `AuthCenterPanel`（设置内）+ 各 `*QuotaFooter` 组件；server 是独立侧边栏页。归属需按 desktop IA 重摆。

### 全局
22. i18n 全量接线（承接主清单 N2；desktop 词条已复制，未全量使用）。
23. 品牌图标集（`BrandIcons.tsx`/`ProviderIcon.tsx`/`iconInference.ts`）未移植——server 类型卡无图标。
24. 设计令牌（字体、间距、圆角、配色、亮/暗主题变量）未对齐。

## 三、对齐原则

- **移植而非重写**：以 desktop `src/components/**` 为唯一 UI 基线，逐组件复制进 `web-src`，只在 runtime 边界做适配（`invokeCommand` → 既有 90 命令 registry / REST）；禁止再扩写自制 Dashboard。
- excluded 功能（MCP/skills/sessions/OpenClaw/Hermes/OMO/桌面窗口控制等）沿用 L0 契约隐藏，顶栏对应图标不渲染。
- 验收沿用人工 checklist（禁止 UI 自动化），以两张截图的区域清单逐项核对。

## 四、Phase U 任务表

| ID | 任务 | 来源（desktop） | 目标（server web-src） | 验收标准 | 工作量 | 依赖 | 状态 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| U0 | 组件体系基座 | desktop `package.json`（shadcn/Radix/lucide/dnd-kit/react-query 等）、`src/components/ui/**`、design tokens（tailwind config/index.css 变量） | web-src 依赖与 desktop 对齐；复制 `components/ui` 与主题变量；建立"desktop 组件移植通道"目录约定（`web-src/src/components` 镜像 desktop 路径） | shadcn/Radix UI 基座、desktop token 层、ThemeProvider 和 Vite build/typecheck/static-checks 通过；ProviderCard 原样迁移留到 U2 | M | — | 已完成（2026-07-04，静态基座） |
| U1 | App 壳移植（去侧边栏） | `src/App.tsx` header/视图切换、`AppSwitcher`、`ProxyToggle`、`FailoverToggle`、`UpdateBadge`、`useAutoCompact` | 移植顶栏布局与视图状态机；侧边栏删除；retained 视图映射：providers（默认）/share/universal/usage/settings；accounts 并入设置 AuthCenter tab（对齐 desktop IA）；excluded 图标不渲染；移除左下调试态 | 已完成：无侧边栏、desktop 风格 logo/设置/Proxy status/Share/Failover/app pills/工具图标/橙色+；Accounts 独立 view/header 图标已移除并迁入 Settings Auth tab；设置/备份/导入图标可跳转 Settings 对应 tab；desktop UpdateBadge 已按 server binary 形态替换为 build/version badge 并链接 About tab；useAutoCompact 为桌面窗口布局专属，server web 暂不渲染；desktop live takeover 写操作在 server 模式下无对应能力，ProxyToggle 以只读运行状态 + Settings/Proxy 入口实现 | L | U0 | 部分完成（2026-07-04，壳 + proxy status + settings tab + build badge） |
| U2 | Provider 主视图 | `providers/ProviderList|ProviderCard|ProviderEmptyState|ProviderActions|HealthStatusIndicator|FailoverPriorityBadge`、dnd 排序 | 以 ProviderList 承接主视图：全宽卡片列表、拖拽排序（接 sort-order shim）、当前 provider 高亮、URL/刷新时间/订阅徽章/账号 email 行内展示；统计卡片行删除；类型矩阵移出主页 | 已完成主页降噪、单列列表、desktop 风格图标/拖拽手柄视觉、URL 行、行内订阅徽章、账号/状态信息、当前 provider 高亮；ProviderList 已补搜索与结果计数，且 toolbar 已抽为 `components/providers/ProviderListToolbar.tsx`；真实 dnd 排序已接入并持久化 provider `sortIndex`；ProviderCard 已按当前排序展示 primary/fallback failover priority badge，`FailoverPriorityBadge` 已抽为独立组件，并在自动故障转移开启时显示/维护 failover queue 成员、队列优先级和 breaker 状态/reset；空态已对齐 desktop `ProviderEmptyState` 的图标、说明和 import/add 双动作，且已抽为 `components/providers/ProviderEmptyState.tsx`；ProviderCard 已补 desktop-style health dot + operational/degraded/failed + latency indicator，`ProviderHealthIndicator` 已抽为独立组件；最近请求摘要与刷新图标已补齐；ProviderActions 已补 duplicate、usage/limits、failover queue/breaker 快捷入口，并在桌面鼠标环境下按 hover/focus 显露，Provider 侧 action button 已复用共享 `components/IconAction.tsx`；ProviderCard adapter/account/quota readiness 已收进高级折叠区，避免主页默认展示 server 诊断矩阵；当前 provider 的 switch/delete 行为已对齐 desktop disabled guard，并为禁用删除补说明 tooltip；组件入口已移动到 `components/providers/ProviderList.tsx`；完整 desktop ProviderCard 行为仍待深化 | L | U1/U10 | 部分完成（2026-07-04，ProviderList search/toolbar split + health/failover badge split + Provider action primitive + ProviderCard + ProviderEmptyState split + dnd sort + actions + current/failover guard + disabled hints + readiness details + desktop-like path） |
| U3 | 添加/编辑对话框 | `AddProviderDialog`/`EditProviderDialog`/`providers/forms/**`（per-app fields + OAuth sections + preset picker）/`ConfirmDialog`/`JsonEditor` | 移植对话框与表单；"可用供应商类型/仅诊断"信息收进添加流程；server 专有字段（adapter readiness 等）放高级折叠 | 已完成首切：添加入口进入 provider catalog modal，presets 与 provider types 在 dialog 内选择；Provider catalog 已补搜索、推荐/A-Z 排序和结果计数；Provider add/edit 表单已增加图标预览、IconPicker 和颜色编辑；已移植 server-local desktop-style `ConfirmDialog` 并替换浏览器原生 confirm，且去掉 Radix Dialog/Button/Checkbox runtime 依赖以恢复 web-dist 余量；已新增轻量 `JsonEditor`/`JsonPreview` 并接入 Provider/Universal/Accounts JSON 表单和各详情预览；Provider form 已新增 authentication/endpoint section，集中 manual token、managed account、base URL/API format；API format 已从自由输入升级为 per-app selector，并保留未知现有值；Full URL 与 endpoint auto-select 已从高级折叠区前移到 endpoint card；Provider form 已补 desktop advanced options：custom User-Agent、local proxy request overrides、Codex Chat reasoning capability，并写入已有 provider meta 字段；Provider edit form 已接入 fetch-models + model catalog 候选选择，模型候选已改为 desktop-style 下拉；provider matrix 已暴露 apiKeyUrl/websiteUrl，API key 字段已补 show/hide 与权威获取链接；完整 desktop per-app form 组件复用仍待深化 | XL | U2 | 部分完成（2026-07-04，catalog search/sort + auth/endpoint controls + native confirm + desktop advanced fields + model fetch/dropdown + API key controls/link metadata） |
| U4 | 设置面板 | `settings/SettingsPage` + `LanguageSettings`/`ThemeSettings`/`GlobalProxySettings`/`ImportExportSection`/`BackupListSection`/`AuthCenterPanel`/`AboutSection` | 移植 tab 式全屏设置；Router/tunnel/upstream proxy 作为新增 tab；主题切换入口生效；退役旧 `SettingsDashboard` 入口 | 已完成 SettingsPage 首切：General/Language/Theme/Directory/Proxy/Failover/Router/Tunnel/Auth/Backup/ImportExport/Diagnostics/About tabs、主题切换入口、server-only tab 归位，支持 App 顶栏图标指定初始 tab；Settings tab 切换已对齐 desktop 行为滚回顶部；General tab 的运行摘要已从 summary tiles 改为 overview status cards；Directory tab 已展示 server config dir/web dist/embedded assets 和主要 store path；About tab 已展示 server build/version/commit metadata；ImportExport tab 已接入 providers/shares/universal JSON 导入导出并补导入确认；Failover tab 已接入 `/api/failover`，支持 per-app 开关、failure threshold、open duration、half-open probes 保存，并展示队列/breaker 摘要；Backup tab 已将备份列表从宽表格改为 snapshot cards；Diagnostics tab 已将 tunnel/share sync 宽表格改为状态卡片；Auth tab 已使用 AuthCenterPanel embedded 模式减少嵌套页面工具栏痕迹，账号卡 action 已复用共享 `IconAction` 的无 wrapper 模式；Settings tab strip 已支持 13 个 tab 横向稳定滚动；Settings 顶层重复页面标题已降级为 owner/runtime 状态文本；API token rotate、router batch sync 已补 destructive/side-effect confirm，API token copy 已补成功/失败反馈；Client tunnel start/stop 已按 runtime status gated；组件入口已移动到 `components/settings/SettingsPage.tsx` 与 `components/settings/AuthCenterPanel.tsx`；完整 desktop `SettingsPage` 组件迁移和 AuthCenter 合并仍待深化 | L | U1 | 部分完成（2026-07-04，tab 壳 + General overview cards + Language/Theme/Directory + About + ImportExport confirm + Failover tab + Backup/Diagnostics cards + Auth embedded + shared action primitive + tab strip + title de-emphasis + destructive/tunnel/router-sync guards + token copy feedback + desktop-like path） |
| U5 | Share 页面 | `share/**`（SharePage/ShareCard/ShareStatsBar/ShareToolbar/ShareRequestLogTable/TunnelConfigPanel/Owner 对话框族） | 以 SharePage 承接 share 视图；connect-info/market/grant/tunnel 等 server 能力接到 desktop 组件对应位置 | 已完成首切：Share stats bar、ShareToolbar 搜索/状态 select/排序 select 已对齐 desktop 三列控件并保留 for-sale 过滤、ShareCard 头部/状态/market badge、binding chips 接入 app/provider 图标；ShareCard actions 已在桌面鼠标环境下按 hover/focus 显露，并复用共享 `IconAction` 的无 wrapper 模式；Owner/market/connect/tunnel 操作已保留；Connect info 已补 direct URL 与 JSON 双复制入口、复制成功反馈和 Clipboard 不可用/失败退化提示；Share export fallback modal 已补 Copy JSON 重试入口和反馈；ShareCard pause/resume/start/stop tunnel 已按 active/paused/stopped 状态 gated；最近 share request log 已从宽表格改为 activity cards；新增 TunnelConfigPanel 汇总 router/tunnel/market 状态并复用 snapshot/restore/edits/markets 动作；Owner change 弹窗已改为 owner handoff 步骤面板；Share 空态已对齐 ProviderEmptyState 的图标、说明和 import/create 双动作；Share 页面级 import/reset usage/restore tunnels/pull edits 已补确认；顶层重复页面标题已降级为紧凑状态文本，保留操作按钮；组件入口已移动到 `components/share/SharePage.tsx`；完整 desktop SharePage 组件复用仍待深化 | L | U1/U10 | 部分完成（2026-07-04，ShareCard/Stats/Toolbar desktop selects + shared action primitive + ConnectInfo/export copy feedback + RequestLogCards/TunnelConfig/OwnerDialog/EmptyState/action guards/import/side-effect confirms + title de-emphasis + desktop-like path） |
| U6 | Usage 页面 | desktop usage 视图（BarChart 图表、日期范围选择） | 替换 UsageDashboard 的文本表为 desktop 图表组件；保留 server 特有过滤参数 | 已完成首切：summary metric cards、轻量 SVG trend chart、range bucket 点击反填 custom range；Usage filter bar 已把 app/range 保持为主控并将 provider/share/user/session/source/health/stream/limit 收进 Advanced filters；range 控件已升级为 desktop-style preset picker，支持 today/1d/7d/14d/30d/all/custom 和 custom live end time；Data Sources 已收敛为 desktop-style compact data source strip，并补 desktop-style source icon mapping；providers/models stats 已从表格改为 desktop-style ranking cards，包含图标、rank、token share、requests/success/cost/latency/last request 摘要；logs/pricing/limits 已从表格堆叠改为 desktop-style activity/pricing/limit cards；UsageDashboard 已支持从 ProviderCard 带 app/provider focus 进入，并对 limits tab 按 provider/app 过滤；pricing delete、usage cost backfill、apply missing default pricing 等写入动作已补确认；顶层重复页面标题已降级为 range/log count 状态文本；后续只剩更深的 desktop usage tab 组件复用与人工核对 | M | U1 | 部分完成（2026-07-04，图表化 + date range picker + compact data sources/icons + provider/model ranking cards + activity/pricing/limit cards + provider focus + pricing confirms + title de-emphasis） |
| U7 | Universal 面板 | `universal/UniversalProviderPanel` | 以 UniversalProviderPanel 承接 universal 视图 | 已完成首切：移除 summary tiles，Universal 卡片改为 desktop `UniversalProviderCard` 风格（品牌图标、providerType、base URL、app chips、hover actions、折叠高级预览），卡片 actions 已复用共享 `IconAction` 的无 wrapper 模式；Universal provider list 已补搜索和 visible/total 计数；Universal 卡片已接入 dnd 排序并持久化 `sortIndex`；Universal 表单已将 Claude/Codex/Gemini 配置收进 per-app section cards；Universal preset modal 已补搜索、recommended/A-Z 排序和结果计数；Universal 空态已对齐 ProviderEmptyState 的图标、说明和 import/preset/create 动作；Universal 表单已补 Save and Sync 行为；Universal 卡片 Sync 已补确认对话框；Universal 页面级 import 已补确认；Universal export fallback modal 已补 Copy JSON 重试入口和反馈；顶层重复页面标题已降级为模板计数状态文本；组件入口已移动到 `components/universal/UniversalProviderPanel.tsx`；完整 desktop `UniversalProviderPanel` 组件复用仍待深化 | M | U1/U10 | 部分完成（2026-07-04，卡片视觉 + shared action primitive + dnd sort + app sections + preset/list search + empty state + save sync + sync/import/export confirm feedback + title de-emphasis + desktop-like path） |
| U8 | 账号/quota 组件 | `AuthCenterPanel` + `*QuotaFooter`（Claude/Codex/Gemini/Cursor/Copilot/Kiro/Ollama/Antigravity/Subscription） | AuthCenterPanel 功能迁入 AuthCenter tab；quota footer 组件按 provider 渲染 | 已完成首切：AuthCenterPanel 嵌入 Settings Auth tab，App 顶栏独立 accounts 入口删除；Auth tab 的账号/能力卡片已接入 provider 图标和 AuthCenter 式卡片头；ProviderCard 已按绑定 account 渲染 quota footer（plan/quota/expiry、进度条、tier 摘要，并补齐 refreshed/next refresh/tier reset/expiry 相对时间与倒计时表达）；Auth tab 新增 AuthCenter overview provider cards，汇总账号数、refresh/quota/import 能力并提供 import 入口；原 Capability Matrix 已改为 Provider readiness cards，保留 login/refresh/quota/import/template 摘要；AccountCard 已补 quota footer（quota readiness、percent meter、tier progress、refresh/error 信息），并补齐 refreshed/next refresh/tier reset 的相对时间与倒计时表达；Copilot/Kiro device flow 已补 user code / verification URL 复制入口和失败退化反馈；AuthCenterPanel 已新增 embedded 模式供 Settings/Auth 复用，隐藏独立页面 toolbar 但保留 refresh/import 行内动作；非 embedded fallback 顶层重复页面标题已降级为账号计数状态文本；完整 desktop `AuthCenterPanel` 和各 provider 专属 quota footer 仍待迁移 | L | U4 | 部分完成（2026-07-04，accounts IA + AuthCenter overview + readiness cards + quota footer relative time + device-flow copy feedback + embedded mode + title de-emphasis） |
| U9 | i18n 全量（承接 N2） | desktop `src/i18n/locales`（已复制） | 移植过程中每个组件保留 desktop 原 `t()` key，不再手写英文字面量；补 JSX 英文字面量扫描进 `static-checks.sh` | JSX 英文字面量静态审计已降为 0 并纳入门禁；ConfirmDialog 默认 cancel/confirm 已补齐四语言；Share modal 标题/按钮/owner handoff/market mode 等英文 fallback 已补四语言短语；Universal import 说明/导入结果和 Usage default pricing 动态 subtitle 已参数化并补四语言短语；ShareCard 状态值 users/private/not loaded 已补翻译；Accounts OAuth/device-flow 结果、credential fallback、readiness/status flags、quota detail summary 与复制反馈已补短语；四语言完整性仍待人工/词条级核对 | 贯穿 U1–U8 | U0 | 部分完成（2026-07-04，静态审计清零 + dialog fallback keys + Share/Universal/Usage/Accounts modal/status phrases） |
| U10 | 品牌图标与主题 | `BrandIcons.tsx`/`ProviderIcon.tsx`/`iconInference.ts`/`ColorPicker`/`mode-toggle` | 移植图标推断与品牌图标；卡片/切换器/preset 均带图标；暗色模式端到端可用 | 已移植 server-local `ProviderIcon`、小型 desktop 图标注册表、图标推断，并接入 App 切换器/ProviderCard/preset 卡片；Provider/Universal 表单均已支持图标预览、IconPicker 与颜色编辑；新增轻量 `ColorPicker`，替换 Provider/Universal 原生颜色字段；图标集已补 GitHub/Google Cloud/Doubao/SiliconFlow/StepFun/Meta/Huawei/NewAPI/SubRouter/ByteDance，以及 ChatGLM/Gemma/ModelScope/Wenxin/Yi/01AI/PaLM/Stability/Midjourney/Vercel/UCloud/Notion/OpenCode/AIHubMix/AICoding/AlgoCode/CatCoder/Claw/Cubence/LongCat/AICodeMirror/CrazyRouter/LionCC/MiCu/PackyCode/RC/SSSAI Code/Xiaomi MiMo；超大或 excluded 图标继续按体积/产品范围跳过 | M | U0 | 部分完成（2026-07-04，品牌图标 + IconPicker + ColorPicker + icon expansion） |
| U11 | 人工核对收口 | 两张基线截图 + `docs/manual-ui-checklist.md` | 按本文件第二节 24 点逐项核对并记录；桌面/移动宽度、亮/暗主题 | 24 点全部勾销或标注豁免理由 | S（人工） | U1–U10 | 待办 |

## 五、执行顺序

```
U0（基座）→ U1（壳）→ U2（Provider 主视图）→ U3（对话框，最大单体）
   U4（设置）/U5（Share）/U6（Usage）/U7（Universal）与 U2/U3 并行切片
   U8（账号入设置）随 U4
   U9（i18n）/U10（图标主题）贯穿全程；U11 收口
```

关键路径：**U0 → U1 → U2 → U3**（主页观感 80% 取决于这四步）。
组件移植期间 desktop 侧若有 UI 提交，按 G3 扫描例行同步。

## 七、实施记录

### 2026-07-04 U0 静态基座

- `web-src/package.json` 已补齐 desktop 非 Tauri UI 依赖：Radix/shadcn、dnd-kit、react-query、react-hook-form、recharts、sonner、cmdk、cva/clsx/tailwind-merge、zod、i18next/react-i18next、CodeMirror 等，为后续组件原样迁移提供编译基础。
- 已复制 desktop `src/components/ui/**`、`src/lib/utils.ts`、`src/index.css` token 层、`tailwind.config.cjs` 和 `components.json`；server 保留自身 `api.ts/runtime.ts/i18n.tsx`。
- `ThemeProvider` 已 server-safe 化：保留 localStorage + `documentElement` light/dark/system 逻辑，移除 Tauri window theme invoke。
- `web-src/src/main.tsx` 已接入 `ThemeProvider` 和 `desktop-theme.css`；`desktop-theme.css` 补 `--panel/--subtle/--success/--warning/--danger` alias，保证现有 server UI 在 U1 前继续可用。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U1 App 壳切片

- `web-src/src/App.tsx` 已删除左侧边栏渲染路径，改为 desktop 风格顶栏：CC Switch 品牌、设置齿轮、Share control、Failover control、Claude/Codex/Gemini segmented pills、Usage/Universal/Share/Backup/Accounts/Import/Sign out/Refresh 图标组和橙色圆形添加按钮。
- Provider app 状态提升到 App 顶栏；`ProviderList` 支持受控 `activeApp`，并通过 `cc-switch-server:add-provider` 事件复用现有添加 provider 流程。
- `HeaderFailoverToggle` 直接对接 server `/api/failover` 与 `/api/failover/apps/:app`；Share control 当前进入 Share 控制面，具体 share tunnel 操作留 U5 组件迁移处理。
- 过渡边界：Provider/Share/Usage/Settings/Universal/Accounts 页面内容仍是 Phase L 自制实现；Accounts 仍保留 header 图标入口，迁入 Settings AuthCenter 留 U4/U8。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；i18n JSX 英文字面量审计保持 0；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider 主页降噪切片

- 移除 Provider 主页中 desktop 不存在的统计卡片行（供应商/可创建类型/仅诊断/健康）和“可用供应商类型”矩阵；可创建类型仍由现有添加按钮进入表单流程，诊断/类型选择后续并入 U3 添加对话框。
- Provider 卡片容器从多列 dashboard 网格改为单列列表，并调整卡片圆角/间距，作为后续移植 desktop `ProviderList/ProviderCard` 的过渡布局。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；i18n JSX 英文字面量审计保持 0；未启动 server，未做 UI 自动化。

### 2026-07-04 U10 品牌图标基础 + U2 ProviderCard 视觉切片

- 新增 `web-src/src/components/ProviderIcon.tsx`、`web-src/src/icons/extracted/**` 小型图标注册表、`web-src/src/config/iconInference.ts` 与 `web-src/src/lib/provider-icons.ts`，复用 desktop `ProviderIcon` 的调用接口，优先覆盖 Claude/OpenAI/Gemini/Ollama/OpenRouter/DeepSeek/智谱/Qwen/Cursor/Kiro/Copilot 及常见云/模型供应商。
- App 顶栏 segmented pills 已从纯色方块切换为品牌图标；Provider 卡片与 preset 卡片已接入图标推断。
- Provider 卡片继续向 desktop 对齐：增加拖拽手柄视觉、图标框、URL 链接行、行内订阅徽章、右上健康/近期请求信息，并将 meta 区压缩为更接近 desktop 的横向信息布局。
- 同步修正 `cc-switch-server:add-provider` 事件监听的依赖边界，避免每次 render 重新注册 handler。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=546646 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Settings tab 壳与主题入口

- `SettingsPage` 从单页堆叠重排为 desktop 风格 tab IA：General/Proxy/Router/Tunnel/Auth/Backup/Diagnostics；现有 server-only 能力没有删除，只按职责归入对应 tab。
- General tab 纳入运行概览、语言设置、runtime readiness，并新增 `ThemeSettingsPanel`，直接调用 server-safe `ThemeProvider` 的 light/dark/system 状态。
- 新增 settings tab 与 theme option 样式，移动端横向滚动 tab，避免 7 个 tab 挤压内容。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U6 Usage 图表化首切

- `UsageSummaryGrid` 从普通 summary tiles 改为 desktop 仪表式 metric cards，显示请求、成功率、tokens、cache hit、成本和进度条。
- `TrendPanel` 从手写纯文本柱状 div 改为轻量 SVG chart，同时展示 tokens/requests/cost 三组指标；点击或键盘激活 bucket 仍会回填 custom range，保留现有 rollup/filter 行为。
- 曾尝试 `recharts`，但 build 后 `webDistBytes=928588 > 900000` 触发内嵌前端体积门禁，因此改为零依赖 SVG 实现，最终 `webDistBytes=555673 < 900000`。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U8 Accounts IA 首切

- `App.tsx` 删除 transitional `accounts` view 和顶栏 KeyRound accounts 图标，减少 desktop 顶栏不一致入口。
- `SettingsPage` 的 Auth tab 嵌入 `AuthCenterPanel`，保留既有 OAuth/device flow/quota 工具能力，同时向 desktop `AuthCenterPanel` 的归属靠拢。
- 增加 `.settings-accounts-card` 嵌入样式，隐藏嵌套账号页自带 toolbar，避免 Settings 卡片内出现双重标题/边距。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U7 Universal 卡片视觉首切

- `UniversalProviderPanel` 移除 desktop 不存在的 summary tiles，列表直接呈现 universal provider cards。
- Universal provider card 对齐 desktop `UniversalProviderCard` 的信息层级：品牌图标、名称、providerType、base URL、app chips 和 hover actions；模型/website/catalog/mapping/raw JSON 改入折叠高级预览。
- Universal preset 卡片接入 `ProviderIcon` 和图标推断，和 Provider preset 视觉一致。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=557544 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 ShareCard/Stats 首切

- Share 页面 summary tiles 改为 desktop `ShareStatsBar` 风格的紧凑 stats bar。
- ShareCard 头部增加 share 图标框、sale/status badge 组合；binding chips 接入 `ProviderIcon`、provider 图标推断和 app fallback 图标。
- 保留现有 share 创建/编辑、ACL、subdomain、connect-info、tunnel、market、import/export 等动作和 modal，不改变 API 行为。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=558987 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Provider catalog 首切

- Provider 添加入口不再直接打开第一个 provider type 表单，改为先进入 `ProviderCatalogModal`。
- Catalog modal 同时展示 desktop presets 和 server-supported provider types，provider type 矩阵正式收进添加流程；选择 type 后再进入现有表单，选择 preset 后走既有 `createProviderFromPreset`。
- Catalog 与 preset/type 卡片统一接入 `ProviderIcon` 和图标推断。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=561190 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U9 i18n 静态审计清零

- 将 `UsageDashboard` 中触发审计的 5 个 JSX prop 英文字面量改为 lower-case translation keys，并继续由组件内部 `tx()` 渲染。
- `scripts/audit-web-i18n-literals.mjs` 当前输出 `web-i18n-literals total=0 max=80 files=`；静态门禁已覆盖该检查。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider 当前高亮

- 新增 `getCurrentProvider(app)` API wrapper，调用既有 `/web-api/invoke/get_current_provider` shim。
- `ProviderList` 按 active app 加载当前 provider id，ProviderCard 增加 `current` class 和 badge；执行 switch 后同步更新本地高亮状态。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=561706 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Provider 高级配置折叠

- Provider 添加/编辑表单保留核心字段在首屏，将 model catalog、model mapping、pricing、advanced JSON 收进 `Advanced configuration` 折叠区。
- 编辑已有高级配置时自动展开折叠区，避免隐藏已配置内容；新建时默认收起，减少 server-only JSON 对主流程的干扰。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=562694 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U7/U10 Universal 图标编辑入口

- Universal provider 表单新增 icon 预览、icon 名称输入和颜色输入，复用 `ProviderIcon` 与图标推断；保存路径已走既有 `icon`/`iconColor` 字段。
- 颜色输入使用原生 color control，避免引入额外依赖或增加 web-dist 体积。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=563737 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U1/U4 Settings 顶栏 tab 跳转

- `SettingsPage` 支持 `initialTab`，并在外部 tab 变化时同步当前 tab。
- App 顶栏设置齿轮进入 General，备份和导入图标进入 Backup tab，减少 desktop 顶栏工具入口与 Settings 面板之间的断层。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=563873 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U8 Accounts AuthCenter 视觉深化

- `AuthCenterPanel` 顶部 summary tiles 改为和 Share/desktop 设置区一致的紧凑 stats bar，避免在 Settings/Auth 内继续呈现 Phase L 管理后台风格。
- Account group、account card、capability card 均接入 `ProviderIcon` 和 `iconInference`，标题区改为 provider 图标框 + 名称/说明 + 状态 badge，更接近 desktop `AuthCenterPanel` 的 provider card 信息层级。
- Device Flow 的 GitHub Copilot/Kiro 入口切换为对应 provider 图标，减少通用手机图标造成的供应商识别差异。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 About tab

- `SettingsPage` 新增 About tab，对齐 desktop 设置页保留版本/关于入口的 IA。
- `loadSettingsPageData()` 接入现有 `/version`，新增 `BuildInfo` 类型；About tab 展示 server name/version/versionLine、commit id/message/time、build time、target/profile/rustc/dirty 状态。
- About tab 明确保持 server 版本定位：不引入 desktop updater、release-notes 自动检查或窗口控制，只展示 server binary build metadata 和源码 commit 链接。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider dnd sort

- `docs/web-runtime-contract.json` 新增 `update_providers_sort_order` shim，并在 `/web-api/invoke/update_providers_sort_order` dispatcher 中落地。
- `ProviderStore` 新增 desktop-compatible `ProviderSortUpdate`，将排序写入 provider JSON 的 `sortIndex` 字段；`/api/providers` 列表按 `sortIndex` + 原始顺序返回，避免前端 reload 后丢失排序。
- `ProviderList` 接入 `@dnd-kit`，Provider 卡片使用真实 sortable item；拖拽结束后乐观更新当前 app 列表，并调用 `updateProvidersSortOrder(activeApp, updates)` 持久化。
- 代价：Vite 报告主 JS chunk 超过 500 kB warning，但总 `web-dist` 体积仍低于 900 kB 门禁。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化，未运行编译型 cargo check。

### 2026-07-04 U4 ImportExport tab

- `SettingsPage` 新增 Import / Export tab，顶栏 Download/Import 图标从 Backup tab 改为打开 ImportExport tab，Backup tab 回到只负责 server 备份快照。
- 新增 provider import/export API wrapper，复用已有 `/api/providers/export` 与 `/api/providers/import`；providers 导入时按 server API 要求只提交 `{ app, provider }`。
- ImportExport tab 提供 providers、shares、universal providers 三组 JSON 面板，支持一键导出到 textarea 和粘贴 JSON 导入；Universal 导出使用既有 `{ providers: [...] }` 格式，同时导入兼容数组、`{ providers }` 与 `{ universal }`。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U3/U10 Provider icon edit

- `ProviderDraft` 新增 `icon` 与 `iconColor` 字段，create/edit 均可编辑；编辑时读取 provider 顶层 `icon`/`iconColor`，保存时写回 provider JSON。
- `ProviderFormModal` 复用 Universal 表单的图标编辑形态，展示 `ProviderIcon` 预览、icon 名称输入和原生 color input；未手动指定 icon 时继续使用 `iconInference` 推断预览。
- 该字段已被现有 `storedProviderIcon()` 消费，因此保存后 ProviderCard、Share binding 等使用 provider icon 的位置会自动显示自定义图标。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=620873 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Share request logs

- `loadSharePageData()` 现在同步加载最近 usage logs，并在 Share 页面按现有 share id 过滤为 share request logs；不新增后端 API。
- Share 页面新增 `ShareRequestLogPanel`，以 desktop request log table 风格展示最近 share 请求的 share/app/model/status/tokens/cost/latency/user/time。
- 现有 Owner/market/connect/tunnel/import/export/share card 行为未改动；日志表只读，作为 SharePage 组件族对齐切片。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=623414 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U7 Universal dnd sort

- `UniversalProviderPanel` 接入 `@dnd-kit`，Universal provider 卡片使用真实 sortable item；拖拽手柄复用 ProviderCard 的 `GripVertical` 视觉和可访问 label。
- 拖拽结束后按当前 Universal 列表乐观更新 `sortIndex`，并通过既有 `saveUniversalProvider()` 持久化每个 provider；失败时展示错误并重新加载远端状态。
- 不新增后端接口，不改变 Universal provider schema；继续使用已有 `sortIndex` 字段和保存路径。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=623414 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 ConfirmDialog

- 新增 server-local `ConfirmDialog`，复用已迁移的 Radix Dialog/Button/Checkbox 基座，并保持与 desktop `ConfirmDialog` 接近的 props：destructive/info variant、alert z-index、可选 checkbox。
- Provider、Universal、Accounts、Share、Settings backup restore、Usage pricing delete 的确认动作已从 `window.confirm` 切换为 `ConfirmDialog`；`web-src` 中浏览器原生 confirm 调用已清零。
- 删除/恢复的实际业务 action 未改变，只把确认交互从浏览器阻塞弹窗迁移到 desktop 风格对话框。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=624565 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 JsonEditor / JsonPreview

- 新增 server-local `JsonEditor`，统一 JSON 文本编辑控件、等宽字体、格式化按钮和内联 JSON parse 校验；不再在各表单中直接散落裸 textarea。
- Provider add/edit 的 modelCatalog/modelMapping/pricing/advanced provider JSON，Universal form 的 per-app modelCatalog/modelMapping JSON，Accounts manual import 的 profile/raw/quota JSON 已接入该组件。
- 新增 server-local `JsonPreview`，替换 Accounts/Universal/Share/Usage 内重复的 preview 函数；Accounts 详情预览继续通过 `redact` 开关脱敏 token/secret/api key/code 等敏感字段。
- 曾评估直接移植 desktop CodeMirror `JsonEditor`，但 Vite build 主 JS 增至 991.31 kB，会破坏 server binary 内嵌前端体积门禁；当前采用零依赖增强 textarea，CodeMirror 版本留待后续拆 chunk 或体积预算调整后再升级。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=672053 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U10 ColorPicker

- 新增 server-local `ColorPicker`，提供颜色色块、HEX 输入、常用色板和重置按钮；继续通过原生 color control 打开系统颜色选择器，不引入新依赖。
- Provider add/edit 与 Universal provider form 的 `iconColor` 字段已从裸 `input type=color` 切换为 `ColorPicker`；保存路径仍写入原有 `iconColor` 字段。
- 新增颜色控件样式，保证色块、HEX 输入和 swatches 在紧凑表单中稳定排列。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=674227 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U8 Provider quota footer

- `ProviderAccountFooter` 从简单 chip 列表升级为 quota footer：展示 account email/id、plan、quota percent、token expiry，并在有 quota percent 时显示进度条。
- 对 `AccountRecord.quota.tiers` 展示最多 3 个 tier，包含 used/limit/unit/reset 信息和每个 tier 的 utilization progress；不新增后端 API，完全复用已有 account quota snapshot。
- 收窄 footer 样式选择器，避免 tier 名称被错误套用错误 chip 样式。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=676333 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Share TunnelConfigPanel

- Share 页面新增 `ShareTunnelConfigPanel`，集中展示 tunnel routes、router sync errors、market access、pending grants，并列出最近同步的 route/subdomain。
- 面板按钮复用现有 snapshot、restore tunnels、pull edits、load markets 动作，不新增后端 API、不改变 share 操作语义。
- 补充响应式样式，窄屏下 tunnel summary 与操作按钮切为单列/换行。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=681241 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U6 Usage provider/model ranking

- Provider stats 与 model stats 表格第一列新增 `UsageRankCell`，展示名称、route/type 摘要、tokens 文本和相对 tokens 进度条。
- 进度条按当前列表最大 tokens 计算，不改变现有 filter、表格列、数据来源或 API。
- 新增 usage ranking 样式，限制列宽和文本溢出，避免长 provider/model 名称撑开表格。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=682659 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U1 Server build badge

- App 顶栏新增 server-safe `HeaderBuildBadge`，读取现有 `/version` build metadata，显示 commit short/version；dirty build 使用 warning 视觉。
- 点击 build badge 进入 Settings/About tab，作为 desktop `UpdateBadge` 在 server binary 形态下的版本状态入口；不引入 Tauri updater/release check。
- `useAutoCompact` 属于桌面窗口尺寸行为，server web 当前保持不渲染，避免引入无效窗口控制逻辑。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=683848 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U6 Usage logs/pricing/limits cards

- Usage Logs tab 从 10 列请求表格改为 activity card 列表：每条请求展示 provider icon、provider/model route、status/time、tokens/cost/latency/source mini metrics、share/user/stream tags 和详情按钮。
- Pricing tab 从纯表格改为 pricing summary + model pricing card grid；每张卡保留 input/output/cache read/cache write 费率和 edit/delete 操作，默认模板/新增定价流程不变。
- Provider Limits tab 从表格改为 limit cards：展示 provider icon、state badge、daily/monthly/quota progress meter、account/share/warning 摘要；API、过滤器、后端数据结构均未变更。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=691261 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Owner handoff dialog

- `OwnerChangeModal` 从普通 key/value + 验证码表单升级为 owner handoff 面板：当前 owner / 新 owner 对照、request code / verify email / save share 三步状态、验证码请求按钮和结果提示集中展示。
- `ModalFooter` 增加独立 `disabled` 参数，避免用 `saving` 表示“验证码为空”时误显示 loading spinner；其他 modal 行为保持不变。
- request code、verify owner、save share 的既有调用链未改变，仍复用 `requestShareOwnerChangeCode()`、`verifyShareOwnerChangeCode()` 和 `saveShare()`。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=694432 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U7 Universal per-app form sections

- `UniversalFormModal` 将原来散落在主 grid 中的 Claude/Codex/Gemini enable toggles 与 model 字段收进 per-app configuration cards；每张卡展示 app icon、启用状态、派生 provider 说明和对应模型字段。
- disabled app 显示紧凑空态说明，enabled app 才展开字段；保存路径仍写入原有 `apps` 与 `models` 字段，不改 API 或 Universal provider schema。
- 样式新增 `universal-app-config-*`，移动端单列，避免三组 app 配置在窄屏中挤压。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=696231 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Provider auth/endpoint section

- `ProviderFormModal` 将原先平铺的 API key、managed account、base URL、API format 字段收进 `ProviderAuthSection`，分成 manual/direct、managed account、endpoint 三张配置卡。
- section header 展示 credential mode、direct/account 支持状态、account 数量和 endpoint format，作为 desktop OAuth/API key sections 的 server-safe 过渡实现。
- 保存逻辑未变：仍写入 `settingsConfig.env`、`settingsConfig.apiFormat`、`meta.authBinding` 等既有字段；provider schema 和 API 均未改动。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=698316 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4/U8 AuthCenter overview

- `AuthCenterPanel` 新增 `AuthCenterOverview`，放在 Settings/Auth 的账号统计之后，按 provider type 展示 Auth Center provider cards。
- 每张卡复用账号、capability、import template 数据，展示账号数、quota readiness、refresh 状态、OAuth/manual import、template 状态和 import 入口；不新增后端 API。
- 该 overview 将 Capability Matrix 的核心信息前置，减少 Auth tab 的管理后台感，同时保留原账号列表、OAuth Preview、Device Flow、manual import 等操作。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=701734 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U10 Icon registry expansion

- 从 desktop 图标集中补入 GitHub、Google Cloud、Doubao、SiliconFlow、StepFun、Meta、Huawei、NewAPI、SubRouter、ByteDance SVG，并接入 server-local `icons/extracted` registry 与 metadata。
- `iconInference` 扩展 GitHub/Copilot、Google Cloud/GCP、Antigravity/AGY、Doubao/Volcengine、SiliconFlow、StepFun、Meta/Llama、Huawei、NewAPI、SubRouter、AWS Bedrock、Ollama Cloud、DeepSeek API 等关键词。
- 图标推断从插入顺序改为长关键词优先，避免 `google cloud` 被 `google`、`githubcopilot` 被 `copilot` 等泛化关键词提前命中。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=720649 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Share request log cards

- `ShareRequestLogPanel` 从 9 列宽表格改为 activity card grid；每条日志展示 app icon、share/model、status/time、app/tokens/cost/latency metrics 和 user/source/stream tags。
- 保留原有 share request log 数据来源、最多 80 条展示限制、share id 到 display name 的映射和空态。
- 新增响应式样式，窄屏下 request log cards 与 metrics 单列显示。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=722416 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider failover priority badge

- ProviderCard 标题行新增 `FailoverPriorityBadge`，按当前 app 的 provider 排序展示 `primary` / `fallback n`，把已持久化的 dnd 排序显式表达为故障转移顺序。
- 该 badge 只读展示，不改 sort-order API、不改 failover 配置；拖拽排序仍通过既有 `updateProvidersSortOrder()` 持久化。
- 样式限制 badge 宽度，避免长 provider 名称和多个状态徽章挤压标题行。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=723099 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U8 Account quota footer

- `AuthCenterPanel` 的 `AccountCard` 新增 `AccountQuotaFooter`，在 Settings/Auth 的账号卡中直接展示 quota readiness、quota percent、subscription、quota refreshed time 和 next refresh。
- 当账号存在 `quota.tiers` 时展示最多 3 个 tier 的 used/limit/unit/reset 摘要与 utilization progress；无 quota 能力且无 snapshot 时不渲染 footer，避免制造空 UI。
- 复用 `AccountRecord` 已有 `quotaPercent`、`quota.tiers`、`quotaRefreshedAt`、`quotaNextRefreshAt`、`lastRefreshError`，不新增 API/schema；样式使用独立 `.account-quota-*` 作用域，避免影响 ProviderCard quota footer。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=726535 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider empty state

- `ProviderList` 新增 server-local `ProviderEmptyState`，对齐 desktop `ProviderEmptyState` 的圆形 Users 图标、居中标题/说明和双操作区。
- 空态的 import 动作接入 App 层 Settings/ImportExport tab；add 动作继续复用现有 provider catalog，不新增 API、不改变 provider 创建流程。
- `.provider-empty-state`、`.provider-empty-icon`、`.provider-empty-actions` 样式独立扩展，保留 `.provider-empty` 给其他页面紧凑空态复用。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=727837 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider health indicator

- `ProviderCard` 右上角从普通 healthy/unhealthy pill 改为 server-local `ProviderHealthIndicator`，对齐 desktop `HealthStatusIndicator` 的状态圆点 + 文案 + latency 形态。
- 基于已有 `ProviderHealth` 计算 `operational` / `degraded` / `failed`：无 health snapshot 或成功率低于 95% 显示 degraded，`healthy=false` 显示 failed；不新增后端字段。
- 样式新增 `.provider-health-*`，保留 recent requests 文本作为辅助信息，避免丢失 server 现有 health 汇总。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=728884 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Provider catalog search/sort

- `ProviderCatalogModal` 新增 desktop preset selector 风格的搜索框，按名称、provider type、api format、base URL、note 同时过滤 presets 和 server provider types。
- 新增 recommended / A-Z 排序切换与结果计数；recommended 保持后端/预设原顺序，A-Z 只在前端展示层排序，不改变创建 API。
- 搜索无结果时分别展示 presets/types 的空态文案；选择 preset/type 的原有流程不变。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=731467 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U10 IconPicker

- 新增 server-local `IconPicker`，从 `icons/extracted/metadata.ts` 读取 desktop 图标元数据，支持搜索、选择、清空和保留自定义 icon 名称。
- Provider add/edit 与 Universal provider form 的 icon 字段从裸文本输入升级为 icon preview + searchable icon grid；仍写入原有 `icon` 字段，不改变 schema。
- IconPicker 与 `ColorPicker` 共处同一 icon editor 区域，保留自动推断 fallback icon/color；样式限制网格与输入宽度，避免撑开 modal。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=734275 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U7 Universal preset search/sort

- `UniversalPresetModal` 新增和 Provider catalog 一致的搜索工具栏，按 preset name、providerType、description、websiteUrl、icon 过滤。
- 新增 recommended / A-Z 排序切换与结果计数；recommended 保持 API 返回顺序，A-Z 仅改变前端展示顺序。
- 搜索无结果时显示紧凑空态；选择 preset 后进入原有 draft 编辑流程，不改变 Universal provider schema/API。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=735461 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Share toolbar

- Share 页面新增 `ShareToolbar`，在 stats bar 后提供 share 搜索、all/active/paused/for sale 分段过滤和当前结果计数。
- 搜索覆盖 share id、displayName、ownerEmail、status、tunnel subdomain、description、sale market、primary binding 和所有 bindings；只改变前端展示列表，不改变后端 API。
- 列表空过滤结果显示紧凑空态；原有 create/import/export/tunnel/market 操作保持在页面主 toolbar。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=738325 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Language/Theme tabs

- `SettingsPage` 新增 `language` 与 `theme` tabs，对齐 desktop SettingsPage 中 LanguageSettings / ThemeSettings 独立 tab 的信息架构。
- General tab 现在只保留运行摘要和 readiness；原语言选择器迁入 Language tab，`ThemeSettingsPanel` 迁入 Theme tab，功能和状态来源不变。
- server-only Proxy/Router/Tunnel/Auth/Backup/ImportExport/Diagnostics/About tabs 保持原位，不改变 settings API。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=738520 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider duplicate action

- ProviderCard actions 新增 duplicate 按钮，对齐 desktop `ProviderActions` 中的复制入口。
- duplicate 通过现有 `saveProvider()` 创建 provider 副本：生成唯一 `id`，名称追加 `copy`，`sortIndex` 放到当前 app 队列末尾；不新增后端 API。
- 复制成功后刷新列表并在新 provider 上展示结果提示；原 edit/test/network/stream/models/switch/delete 行为不变。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=739396 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 ProviderList search

- Provider 主视图新增 `ProviderListToolbar`，提供 provider 搜索和 visible/total 结果计数，对齐 desktop ProviderList 的搜索体验。
- 搜索覆盖 provider id/name/type、model、base URL、api format、managed account email/subscription 和 category；只影响前端展示，不改变 providers API。
- DND 排序仍按完整 active app provider 队列计算 priority 和 sort order；过滤无结果时显示紧凑空态。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=741301 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Directory tab

- `SettingsPage` 新增 `directory` tab，对齐 desktop SettingsPage 的 Directory 信息架构。
- Directory tab 复用 `/web-api/context` 的 runtime 信息展示 config dir、web dist dir、embedded web assets 数量和主要 store 文件路径；不新增后端 API。
- `refresh()` 同步加载 settings page data 与 runtime context；context 失败时只影响 Directory tab 的只读详情，不阻塞 settings 主数据加载。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=743588 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2/U6 Provider usage quick action

- ProviderCard actions 新增 `Usage and limits` 图标入口，对齐 desktop `ProviderActions` 中的 usage/configure usage 动作位；server 版本不引入 UsageScriptModal，而是跳转到现有 Usage 页面。
- `App.tsx` 新增 usage focus 状态；从 ProviderCard 进入 Usage 时携带 app、providerId 和目标 tab，顶栏 Usage 入口则清空 focus 保持普通全局用量视图。
- `UsageDashboard` 支持 `initialFocus`，自动填充 app/provider filter 并切到 logs 或 limits；Provider 有 limit snapshot 时优先打开 limits tab，否则打开 logs tab。
- Provider limits tab 新增前端过滤，按当前 app/providerId（含 provider name/type、account、share 摘要）收窄展示，避免 ProviderCard 跳转后仍显示全部 limits。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=744475 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider health refresh row

- `ProviderHealth` web 类型补齐后端已返回的 `lastRequestAtMs` 字段，前端不再只能显示 recent request 数量。
- ProviderCard 右上健康区升级为 desktop-style health stack：状态圆点/文案、最近请求相对时间、请求数摘要和刷新图标按钮。
- 刷新图标复用现有 config test action，不新增后端 API；原 action 区的 config test 按钮保留，避免隐藏已有测试能力。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=745653 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Settings tab scroll reset

- `SettingsPage` 补齐 desktop `SettingsPage` 的 tab scroll reset 行为：active tab 变化后通过 `useLayoutEffect` 将 settings 容器滚回顶部。
- 该切片只调整交互状态，不改 settings API、不改各 tab 内容，也不引入 UI 自动化。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=745728 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U10 Icon registry second expansion

- 对照 desktop `src/icons/extracted`，在 server-local registry 中补入 20 个体积较小的 SVG 图标：ChatGLM、Gemma、ModelScope、Wenxin、Yi、01AI、PaLM、Stability、Midjourney、Vercel、UCloud、Notion、OpenCode、AIHubMix、AICoding、AlgoCode、CatCoder、Claw、Cubence、LongCat。
- `metadata.ts`、`index.ts` 和 `iconInference.ts` 同步扩展，IconPicker 搜索、ProviderIcon 渲染和 provider/preset 自动推断都能使用新增图标。
- 明确跳过 `ccsub.svg`、`dds.svg` 等 MB 级 desktop SVG，避免突破 server binary 内嵌 web-dist 体积门禁。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=781089 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider hover actions

- ProviderCard actions 在 desktop 鼠标环境下改为 hover/focus-within 显露，对齐 desktop `ProviderCard` 的 hover action 行为，减少卡片默认状态的管理后台感。
- 样式 scoped 到 `.provider-card > .provider-actions`，避免影响 Usage pricing card 等其他复用 `.provider-actions` 的区域；触屏设备保持常显，避免移动端无法操作。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=781335 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U8 Account quota relative time

- Account quota footer 的 refreshed 时间从纯绝对时间改为 `refreshed {{time}}` 相对表达，接近 desktop quota footer 的查询时间反馈。
- `quotaNextRefreshAt` 改为显示 `in {{time}}` 倒计时，并保留绝对时间在 title；tier reset 也改为 `resets in {{time}}`，过期或异常时回退绝对时间。
- 实现只消费已有 `quotaRefreshedAt`、`quotaNextRefreshAt`、`quota.tiers[].resetsAt`，不新增后端 API/schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=782260 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 ShareCard hover actions

- ShareCard 操作区复用 `.provider-actions`，本轮补齐 desktop-like hover/focus 显露行为，减少 share list 默认状态的信息噪声。
- 样式 scoped 到 `.share-card > .provider-actions`；触屏设备保持常显，避免移动端操作不可达。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=782369 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Provider API format selector

- Provider add/edit 的 endpoint card 中，API format 从裸文本输入改为 per-app selector：Claude 提供 Anthropic/OpenAI Chat/OpenAI Responses/Gemini Native，Codex 提供 Responses/Chat，Gemini 提供 Gemini Native/OpenAI Chat。
- selector 会自动包含 matrix default 和当前 draft value，避免编辑历史/未知格式时丢失配置。
- 保存路径仍写入既有 `settingsConfig.apiFormat` 和 `meta.apiFormat`，不改 provider schema 或后端 API。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=782925 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U6 Usage advanced filters

- Usage filter bar 保留 app/range/custom date 作为首层主控，将 Provider ID、Share ID、User email、Session ID、Data source、Health check、Stream status、Limit 收进 `Advanced filters` 折叠区。
- 折叠区显示 active filter 计数；高级条件存在时默认展开，并提供 `Clear advanced filters` 快捷清理。
- 该切片只调整前端过滤 UI 结构，`filterFromDraft()` 和 usage API 查询参数保持不变。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=784570 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U7 Universal provider list search

- Universal provider list 新增 `UniversalListToolbar`，提供搜索输入和 visible/total 计数，对齐 ProviderList/ShareToolbar 已完成的列表筛选体验。
- 搜索覆盖 universal provider id/name/type、base URL、website、notes、icon、enabled apps 和各 app model 配置；只影响前端展示。
- DND 排序仍按完整 universal provider 队列计算和持久化，过滤只改变当前渲染列表；搜索无结果显示紧凑空态。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=785643 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Provider desktop advanced fields

- Provider add/edit 表单新增 `Desktop advanced options` 折叠区，对齐 desktop per-app form 中的 endpoint behavior、custom User-Agent、local proxy request overrides 和 Codex reasoning capability 字段。
- 新字段双向映射到已有 provider `meta`：`isFullUrl`、`endpointAutoSelect`、`customUserAgent`、`localProxyRequestOverrides`、`codexChatReasoning`；留空时删除字段，request overrides 只更新 headers/body 并保留已有其他 override 字段。
- Codex reasoning capability 只在 Codex provider 表单展示，并仅在 `openai_chat` API format 下保存；保存前会解析 headers/body JSON 且要求为对象，User-Agent 会拒绝控制字符，避免写入无效 meta。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=859434 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4/U8 AuthCenter embedded mode

- `AuthCenterPanel` 新增 `embedded` 模式，Settings/Auth tab 使用嵌入模式渲染，减少 Settings card 内再出现独立 Accounts page toolbar 的信息架构断层。
- embedded 模式隐藏独立页面标题区，但保留 refresh/import 作为 `auth-center-inline-actions` 行内动作；独立 AuthCenterPanel 入口行为不变。
- 删除 `.settings-accounts-card .provider-toolbar { display: none }` 的 CSS hack，改由组件语义控制 DOM；样式新增 `.auth-center-panel.embedded` 和 `.auth-center-inline-actions`。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=793904 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5/U7 Share and Universal empty states

- Share 页面空态替换为 desktop-style `ShareEmptyState`：圆形图标、说明、provider 缺失提示，以及 import/create 双动作；create 仍复用现有 share draft，provider 不存在时禁用。
- Universal 页面空态替换为 `UniversalEmptyState`：圆形图标、说明、import/from preset/create 三动作；preset 不存在时仅禁用 preset 动作，手动 create 保持可用。
- 该切片只调整空态渲染和已有前端动作入口，不新增 API、不改变 share/universal provider schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=795381 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U10 Icon registry third expansion

- 对照 desktop `src/icons/extracted`，补入第三批仍在 server web-dist 体积预算内的 SVG：AICodeMirror、CrazyRouter、LionCC、MiCu、PackyCode、RC、SSSAI Code、Xiaomi MiMo。
- `metadata.ts`、`index.ts` 和 `iconInference.ts` 同步扩展；`rc` 图标只进入 IconPicker/手动选择，不加入自动推断关键词，避免两个字符的 `rc` 误命中普通 provider 文本。
- 继续跳过 `ccsub.svg`、`dds.svg`、`shengsuanyun.svg` 等超大 SVG，以及 MCP 这类 server 明确不需要的功能范围图标，保留 server binary 内嵌前端体积预算。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=859434 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U1 Header Proxy status

- App 顶栏开关组新增 `HeaderProxyStatus`，读取现有 `get_proxy_status` web runtime shim，运行中使用 desktop mini toggle active 视觉，title 展示 status/mode/baseUrl。
- 点击 Proxy status 进入 Settings/Proxy tab；不提供 desktop live takeover 写开关，因为 server runtime 本身就是 HTTP proxy，当前 `get_proxy_takeover_status` shim 固定返回 false，没有可写 takeover 后端能力。
- 保留 Share 与 Failover 入口：Share 继续负责 share 页面/路由管理，Failover 继续使用现有 `/api/failover/apps/:app` 写接口。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=860324 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 ProviderActions current guard

- ProviderCard 主 switch 动作对齐 desktop `ProviderActions`：当前 provider 显示 disabled secondary `current` 状态，非当前 provider 使用 primary switch 动作和 Play 图标。
- 当前 provider 的 delete 动作禁用，避免 UI 层允许删除正在使用的 provider；后端 API 不改动，非当前 provider 的 delete/confirm 流程保持不变。
- `IconAction` 增加 `disabled` 语义，继续复用 busy spinner 和 danger 样式。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=860520 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Settings tab strip

- Settings 已扩展为 12 个 tab（General/Language/Theme/Directory/Proxy/Router/Tunnel/Auth/Backup/ImportExport/Diagnostics/About），原固定 `repeat(7)` grid 会让桌面 tab 区换行断层。
- `.settings-tabs` 改为单行 flex tab strip，按钮固定最小宽度并横向滚动；移动端继续使用相同滚动模型，只调整最小宽度。
- 该切片只调整 Settings tabs 布局，不改 settings API、tab 状态或内容。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=860471 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Provider model fetch field

- Provider add/edit 表单的 Model 字段升级为 desktop `ModelInputWithFetch` 的 server-local 轻量形态：从已保存 `modelCatalog.models` 中提供 datalist 候选，保留自由输入。
- 编辑已有 provider 时显示 fetch 按钮，复用现有 `/api/providers/:id/fetch-models` 且 `merge=true`；成功后将后端返回的 merged provider 同步回 `modelCatalogJson` 与 `advancedJson`，并在空 model 时填入首个候选。
- 创建态不显示 fetch 按钮，避免无 provider id 时制造临时保存流程；不新增后端 API、不改变 provider schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=860471 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U7 Universal Save and Sync

- Universal 表单 footer 新增 `Save and Sync`，对齐 desktop `UniversalProviderPanel` 的保存后同步行为；普通 `Save Universal` 保持只保存。
- 实现复用现有 `saveUniversalProvider()` 与 `syncUniversalProvider()`，通过 submit mode 区分保存路径；同步结果沿用 `syncSummary()` 展示，不新增后端 API、不改变 Universal provider schema。
- 创建和编辑态均可直接 Save and Sync；列表卡片原有 Sync、Duplicate、Delete 行为保持不变。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=862608 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U7 Universal sync confirm

- Universal 卡片的 Sync 操作从直接执行改为先展示 `ConfirmDialog`，对齐 desktop `UniversalProviderPanel` 的同步确认流程。
- 确认文案明确会同步到 enabled apps，且可能覆盖派生 provider；确认后仍调用原有 `syncUniversalProvider()` 路径，不新增后端 API。
- Delete confirm 与 Save and Sync 行为保持不变；本切片只降低误触同步风险。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=863155 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Share action guards

- ShareCard 的 Pause/Resume、Start tunnel、Stop tunnel 按现有 share `enabled/status` 做前端 guard：active 才能 pause/stop，paused/stopped 才能 resume/start。
- expired/exhausted 等状态不直接启用恢复/启动入口，避免 UI 暗示可以绕过过期或额度耗尽状态；Reset usage、Edit、ACL、Subdomain、Connect info、Market、Delete 行为保持不变。
- `IconAction` 增加 disabled 语义，复用原有 busy/danger 样式，不新增后端 API、不改变 share schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=863412 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Settings API token confirm

- Settings/Auth 的 `Rotate API token` 从直接执行改为先展示 destructive `ConfirmDialog`，说明现有客户端 token 会失效。
- 确认后仍调用现有 `rotateApiToken()` 和 result/token preview 路径；Backup restore 原有确认保持不变。
- 本切片只补 destructive action guard，不新增后端 API、不改变认证存储。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=863485 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Client tunnel action guards

- Settings/Tunnel 的 Start 与 Stop client tunnel 按现有 runtime status 做前端 guard：running/starting/connecting/renewing/leasing/retrying 等非 stopped/end/error 状态禁用 Start，非运行状态禁用 Stop。
- Claim 和 tunnel config save 行为保持不变；本切片只避免重复启动或停止未运行 tunnel，不新增后端 API。
- `ActionButton` 增加 disabled 语义，继续复用现有 busy spinner 和按钮样式。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=863789 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 ImportExport confirm

- Settings/ImportExport 的 providers/shares/universal import 从直接执行改为先展示 `ConfirmDialog`，说明同 ID 记录可能被更新。
- 导出、textarea、JSON 解析和原有 `importData()` 路径保持不变；确认后才执行导入，不新增后端 API、不改变 import schema。
- 三类 ImportExportCard 共用同一确认逻辑，避免 providers/shares/universal 行为分叉。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=864217 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5/U7 Page import confirm

- Share 页面级 `ImportSharesModal` 与 Universal 页面级 `ImportUniversalModal` 从“解析成功后直接写入”改为“解析成功后进入 `ConfirmDialog`，确认后才调用原 import action”。
- 确认文案显示即将导入的记录数，并提示相同 ID 的现有记录可能被更新；Settings/ImportExport 与页面级 import 的风险提示保持一致。
- 原 JSON 解析、modal 关闭、导入 API、刷新列表和 result 展示路径保持不变；不新增后端 API、不改变 shares/universal provider schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=864870 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Share side-effect confirms

- ShareCard 的 `Reset usage` 从直接执行改为先展示 destructive `ConfirmDialog`，确认后才调用原 `resetShareUsage()` action。
- Share toolbar 的 `Restore tunnels` 与 `Pull edits` 从直接执行改为先确认，分别提示可能替换 tunnel runtime state 或更新匹配 share。
- Snapshot、load markets、connect info 等只读/刷新动作保持直接执行；不新增后端 API、不改变 share/tunnel schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=865621 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U6 Usage bulk-action confirms

- Usage toolbar 的 `Backfill costs` 从直接执行改为先展示 `ConfirmDialog`，提示历史 cost 可能被当前 pricing rules 重新计算。
- Pricing defaults modal 的 `Apply Missing` 从直接批量写入改为先确认，提示会创建缺失默认 pricing 并可能 backfill usage costs。
- 单个 pricing add/edit/apply 仍保留表单/按钮直接提交；delete confirm 继续沿用既有实现。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=866147 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Router batch sync confirm

- Settings/Router 的 `Batch sync` 从直接执行改为先展示 `ConfirmDialog`，提示远端 router 中匹配 share 记录可能被更新。
- Register、heartbeat、claim、start/stop tunnel 等显式单动作保持原行为；本切片只覆盖批量远端同步风险点。
- 原 `batchSyncRouterShares()` API、result 展示和 busy key 均不变；不新增后端 API、不改变 router config schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=866449 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U8 Provider quota footer relative time

- ProviderCard 的账号 quota footer 从仅展示绝对 expires/tier reset 日期，深化为显示 `expires in/expired`、`refreshed ... ago`、`refresh in` 与 tier `resets in` 倒计时。
- 完整时间保留在 chip `title`，主界面保持紧凑 chip + meter + tier layout；Accounts/Auth quota footer 既有相对时间行为保持不变。
- 只消费已有 `AccountRecord.expiresAt/quotaRefreshedAt/quotaNextRefreshAt/quota.tiers[].resetsAt` 字段；不新增后端 API、不改变 account schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=867732 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider failover queue action

- `loadProviderListData()` 增加现有 `/api/failover` snapshot，ProviderList 按当前 app 读取 failover config 与 provider queue。
- 自动故障转移开启时，ProviderCard 显示 `failover Pn` 队列优先级，并在 hover actions 中提供加入/移出 failover queue 的快捷按钮，对齐 desktop `ProviderActions` 的 failover queue 操作位。
- 队列更新复用现有 `PUT /api/failover/apps/:app`，只提交新的 `providerQueue` 并保留当前 enabled 状态；不新增后端 API、不改变 failover schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=869032 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider failover breaker reset

- ProviderList 复用同一个 `/api/failover` snapshot 中的 breaker 列表，ProviderCard 在 failover 开启且 breaker 非 `closed` 时显示 `breaker open/half_open` 状态徽章。
- ProviderCard hover actions 新增 `Reset failover breaker`，调用现有 `POST /api/failover/providers/:provider_id/reset?app=...` 并更新本地 breaker snapshot。
- 成功后沿用卡片 result 行提示；不新增后端 API、不改变 breaker/failover schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=869901 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Settings Failover tab

- `SettingsPage` 新增 Failover tab，读取现有 `/api/failover` snapshot 和 `/api/providers` provider 列表，补齐 desktop Proxy/Failover 设置面的 server-safe 配置入口。
- 每个 app（Claude Code/Codex/Gemini）提供自动故障转移开关、failure threshold、open duration seconds、half-open probes 编辑，并通过现有 `PUT /api/failover/apps/:app` 保存；provider queue 保持只读摘要，队列成员继续由 ProviderCard 快捷动作维护。
- Failover tab 展示 enabled apps、queued providers、open breakers 总览，以及 per-app queue Pn 和非 closed breaker 摘要；不新增后端 API、不改变 failover schema。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=876277 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U6 Usage provider/model ranking cards

- `ProviderRankingGrid` 与 `ModelRankingGrid` 退役旧 table 渲染，改为 usage ranking card grid；每张卡展示 provider/model 图标、rank、token share meter、requests/success/cost/latency/last request 等摘要。
- 数据来源、usage filter、stats API、排序和 limit 逻辑均保持不变，只调整展示结构，减少 Usage 页面残留的管理后台宽表格形态。
- 新增 `.usage-ranking-*` 样式并补移动端 header/footer 单列规则，避免长 provider id、model route 或 pricing model 撑开布局。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=877865 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Diagnostics cards

- Settings/Diagnostics 的 tunnel diagnostics 与 share sync diagnostics 从宽表格改为状态卡片，展示 key/name、status pill、URL/subdomain/lease/sync/error 等字段。
- 新增 `diagnosticTone()` 将 error/disabled/stopped/failed 等状态映射为 success/warning/danger，保留原 diagnostics API 与 summary 计算口径。
- 新增 `.diagnostics-card-*` 样式，长 URL、share id 和 error 文本保持截断，不再要求 Settings tab 横向表格滚动。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=878901 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Backup snapshot cards

- Settings/Backup 的 backup manifest 列表从 table 改为 snapshot cards，展示 backup id、created time、reason、files、size 和 stored files 摘要。
- Restore 按钮继续调用原 `onRestore`，仍先进入既有 `ConfirmDialog`；backup create、policy summary 和 backup API 均未改变。
- 新增 `.backup-card-*` 样式，长 backup id / stored files 走截断，减少 Settings tab 宽表格残留。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check`、残留扫描和旧表格样式扫描均通过；`webDistBytes=879157 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U8 Auth readiness cards

- `AuthCenterPanel` 中原 `Capability Matrix` 面板改为 `Provider readiness` provider cards，减少 Auth tab 的矩阵/后台语义。
- 每张 readiness card 保留 provider icon、状态、login/refresh/quota 摘要、serverNativeStage/quotaStrategy/import 标记和 Import template details；Import 按钮仍调用原 `onImport(providerType)`。
- 新增 `.auth-readiness-*` 样式，复用 AuthCenter 卡片的圆角、hover 和三列 metrics 视觉；不改账号/OAuth/device-flow/quota API。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check`、残留扫描和旧 matrix/table 样式扫描均通过；`webDistBytes=880243 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Settings overview cards

- Settings/General 的 owner/router/tunnel/pending logs/backups 摘要从 `summary-tile` 改为 `SettingsOverviewStrip` status cards，展示 value、detail 和 success/warning/danger 状态。
- 删除 Usage 中未使用的 `SummaryTile` helper，并清理 `summary-tile`、`provider-summary-row`、`settings-summary-row` 等旧样式残留。
- 新增 `.settings-overview-*` 样式和移动端单列规则；不改 settings API、readiness 计算或任何写操作。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check`、残留扫描和旧 summary/matrix/table 样式扫描均通过；`webDistBytes=881127 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider readiness details

- ProviderCard 内 adapter/account/quota readiness 从默认展开卡片改为 collapsed details 区块，默认只展示 `Adapter readiness`、creatable/diagnostic 状态和 credential mode。
- 展开后继续展示 direct/account/managed/refresh/quota/plan flags 与 serverNativeStage/status/note；provider matrix、capability 数据源和 API schema 均不变。
- 新增 `.provider-readiness-panel` details/summary 样式，减少 Provider 主列表默认的 server 诊断信息密度。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check`、残留扫描和旧 summary/matrix/table 样式扫描均通过；`webDistBytes=881127 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5/U6/U7 top toolbar title de-emphasis

- Share、Usage、Universal 内容区顶层 `provider-toolbar` 移除重复的页面级 icon + h2 标题；当前视图标题继续由 App desktop header 承担。
- 原副标题数据保留为紧凑 `.provider-toolbar-status`：Share 显示 route count，Usage 显示 range/log count，Universal 显示 template count；刷新、导入、导出、preset、backfill 等操作按钮不变。
- 目标是继续消除 Phase L 页面大标题残留，同时保留 server-only 操作入口。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和重复标题/旧残留扫描均通过；`webDistBytes=881341 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 Settings top toolbar title de-emphasis

- Settings 内容区顶层 `provider-toolbar` 移除重复的页面级 icon + h2 标题；当前视图标题继续由 App desktop header 承担。
- owner email / runtime subtitle 保留为紧凑 `.provider-toolbar-status`，刷新、错误和 result 展示不变。
- 与 Share/Usage/Universal 同一规则，继续收敛 Phase L 页面大标题。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和重复标题/旧残留扫描均通过；`webDistBytes=881341 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U8 Accounts fallback toolbar title de-emphasis

- AuthCenterPanel 的非 embedded fallback 顶层 `provider-toolbar` 移除重复的页面级 icon + h2 标题；Settings/Auth embedded 入口继续隐藏该 toolbar。
- imported account count 保留为紧凑 `.provider-toolbar-status`，refresh/import 操作不变。
- 删除不再需要的 `KeyRound` import，减少独立 Accounts 页面残留的 page 标题形态。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和重复标题/旧残留扫描均通过；`webDistBytes=881341 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 stale dashboard CSS cleanup

- 删除无组件引用的 `.share-request-log-table` 宽表格样式和 `.universal-summary-row` summary grid 样式，避免旧 Phase L table 命名继续残留在 stylesheet。
- 不改变运行时代码或 API，只清理已经由 activity cards / universal cards 替代后的死样式。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和旧 CSS 残留扫描均通过；`webDistBytes=881341 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 Usage ranking component rename

- Usage provider/model ranking 已是 card grid，组件名从 `ProviderStatsTable` / `ModelStatsTable` 改为 `ProviderRankingGrid` / `ModelRankingGrid`。
- 不改变渲染结构或数据流，只清理旧 table 语义命名，避免后续 residual scan 继续命中已退役表格实现。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和旧 table 命名残留扫描均通过；`webDistBytes=881341 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 Usage limits component rename

- Usage provider limits 已是 limit cards，组件名从 `ProviderLimitsTable` 改为 `ProviderLimitsGrid`。
- 不改变渲染结构或数据流，只清理旧 table 语义命名，避免后续 residual scan 继续命中已退役表格实现。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和旧 table 命名残留扫描均通过；`webDistBytes=881341 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 Backup snapshot component rename

- Settings backup manifest 列表已是 snapshot cards，组件名从 `BackupTable` 改为 `BackupSnapshotGrid`。
- 不改变渲染结构或数据流，只清理旧 table 语义命名，避免后续 residual scan 继续命中已退役表格实现。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和页面级 table 命名残留扫描均通过；`webDistBytes=881341 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U6 Usage date range picker

- Usage filter bar 的 range 区从普通 segmented select 升级为轻量 `UsageRangePicker`，对齐 desktop `UsageDateRangePicker` 的 preset 入口形态。
- Preset 扩展为 today/1d/7d/14d/30d/all/custom；默认从旧 24h 语义迁到等价的 `1d`，避免默认查询窗口变化。
- Custom range 继续使用 `datetime-local`，并新增 `Live end time` toggle；开启时清空 `customTo`，沿用既有 open-ended query 语义。
- 新增 `.usage-range-*` 和移动端规则，避免 7 个 preset 在窄屏挤压；不引入 Popover/calendar 依赖，保持 web-dist 体积门禁。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check`、i18n literal/residual scan 均通过；`webDistBytes=881341 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Share toolbar sort

- ShareToolbar 在现有 search/status/for-sale filter 基础上补齐 desktop `ShareToolbar` 的 sort 控件，支持 created time desc、expires time asc、tokens used desc、name asc。
- 排序只作用于前端展示列表，不改变 shares API、创建/编辑/market/tunnel 行为；`ShareRecord` web 类型接入可选 `createdAtMs` / `createdAt` / `created_at_ms` / `created_at`，排序时统一归一化为毫秒，缺失时保持原相对顺序。
- 新增 `.share-sort` 样式，和 search/filter/count 保持同一 toolbar 密度，避免把排序恢复成管理后台表格形态。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 和残留扫描均通过；`webDistBytes=884761 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Share status filters

- ShareToolbar 的 status filter 补齐 desktop `ShareToolbar` 中的 expired/exhausted 选项，现支持 all/active/paused/expired/exhausted。
- server-only `for sale` 作为独立 filter 保留；过滤只作用于前端展示列表，不改变 shares API、market、tunnel 或 owner handoff 行为。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 均通过；`webDistBytes=884761 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U6 Usage data source strip

- Usage 的数据来源区域去掉 Phase L `h2` 标题块，改为 compact `Data Sources` label + source chips，贴近 desktop `DataSourceBar` 的横条密度。
- 现有 source aggregation、点击筛选、loading 状态和 server usage API 均保持不变；desktop 的 local CLI session sync 属于 excluded `localCliSessions`，server 继续不渲染。
- 新增 `.usage-data-source-label` 样式并微调 chip padding/background，窄屏下仍保持单列。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 均通过；`webDistBytes=885073 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U6 Usage data source icons

- DataSource chip 图标从粗略 `includes("session")` 判断改为 desktop-style source mapping：session_log/codex_session/gemini_session/opencode_session 使用 FileText，proxy/codex_db/未知来源使用 Database。
- `all` 聚合 chip 使用 Database，保留现有 source aggregation、点击筛选和 server usage API。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 均通过；`webDistBytes=885195 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider disabled action hint

- ProviderCard 的 `IconAction` 支持 disabled-specific tooltip，并用外层 wrapper 承接 hover；当前 provider 的 Delete 按钮继续禁用，但 tooltip 从普通 `Delete` 改为 `Current provider cannot be deleted`。
- 删除 guard、ConfirmDialog、provider API 均不改变；只补齐 desktop `ProviderActions` disabled hint 的交互细节。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 均通过；`webDistBytes=885361 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Provider API key / model controls

- Provider add/edit 表单的 API key 区域补齐 desktop `ApiKeySection` 的核心交互：show/hide API key、直连/托管账号提示和保守推断的 `Get API Key` 链接；不改变 provider 保存路径。
- Model 字段从 datalist 升级为 server-local desktop-style catalog dropdown：输入仍可自由编辑，有 `modelCatalog` 候选时通过下拉选择；fetch models 继续复用现有 `/api/providers/:id/fetch-models?merge=true`。
- 新增 `.provider-field`、`.provider-api-key-*`、`.provider-model-menu` scoped 样式，避免把按钮嵌在 `<label>` 内造成表单语义问题。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 均通过；`webDistBytes=890878 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Provider matrix API key links

- `ProviderMatrixEntry` / `ProviderTypeSummary` 新增可选 `apiKeyUrl` 与 `websiteUrl` 字段，由 provider type 集中提供；前端 `Get API Key` 优先读取 matrix 权威字段，启发式 URL 只作为兼容 fallback。
- 该字段只影响 web 表单提示，不改变 provider 保存 schema、proxy 转发、OAuth capability 或真实 provider 验收边界。
- 顺手修复 `cargo check --all-targets` 暴露的 `claim_client_tunnel` 分支 unused `Json` warning，显式丢弃只为副作用执行的 update 返回值。
- 验证：`cargo fmt -- --check`、`cargo check --all-targets`、`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 均通过；`webDistBytes=890912 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 ConfirmDialog native runtime

- `ConfirmDialog` 从 shadcn/Radix `Dialog`/`Button`/`Checkbox` 切换为 server-local 原生 React/HTML 实现，保留现有 props、ESC/backdrop cancel、checkbox confirm 和 destructive/info variant。
- 新增 `.confirm-dialog-*` 与 `.danger-button` scoped 样式，继续保持 desktop-style 对话框外观；所有调用点保持不变，仍不回退到 `window.confirm`。
- 该切片不改变业务动作，只移除未必要的 Radix runtime 依赖，给后续 UI parity 恢复体积余量。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 均通过；`webDistBytes=849546 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U3 Endpoint controls placement

- Provider form 的 `Treat Base URL as full upstream URL` 与 `Auto-select fastest endpoint` 从 `Desktop advanced options` 折叠区前移到 `Authentication and endpoint` 的 endpoint card 内，对齐 desktop `EndpointField` 将 endpoint 行为放在核心 endpoint 控件附近的结构。
- `Desktop advanced options` 现在只保留 request overrides 与 Codex Chat reasoning 等真正高级配置；保存仍写入既有 `meta.isFullUrl` / `meta.endpointAutoSelect`，不改 provider schema 或后端 API。
- 新增 `.provider-endpoint-toggle-row` 样式，保证两个 toggle 在 endpoint card 中稳定排列。
- 验证：`cargo fmt -- --check`、`cargo check --all-targets`、`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh`、`git diff --check` 均通过；`webDistBytes=849353 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U9 ConfirmDialog fallback i18n

- 轻量 runtime i18n 的 `common` namespace 补齐 `cancel` / `confirm` 四语言翻译，避免 `ConfirmDialog` 未传 `cancelText` 或 `confirmText` 时回退显示 `common.cancel` / `common.confirm` key。
- 该修正只影响默认按钮文案，不改变 ConfirmDialog 调用点或任何 destructive action guard。

### 2026-07-04 U5 Share connect info copy URL

- ShareCard 的 `Connect info` 详情在原 `Copy JSON` 外新增 `Copy URL`，可直接复制 `directUrl`，对齐 desktop ShareCard 的连接信息复制入口。
- 保留现有 connect-info API、JSON preview 和 clipboard fallback 边界；不改变 share/tunnel/router 行为。

### 2026-07-04 U5 Share connect info copy feedback

- ShareCard 的 `Copy URL` / `Copy JSON` 改为统一复制处理，成功后在卡片内显示 `Copied URL` / `Copied JSON` 状态，贴近 desktop `handleCopy + toast` 的即时反馈。
- Clipboard API 不可用或写入失败时显示可见退化提示，引导用户手动复制已经展示的 direct URL / JSON；不改变 connect-info API、JSON preview 或 share/tunnel/router 行为。

### 2026-07-04 U4 Settings API token copy feedback

- Settings/Auth 的 `Copy Token` 不再静默调用 `navigator.clipboard`，改为显示 `API token copied` 成功状态，并在 Clipboard API 不可用或写入失败时显示手动复制退化提示。
- 轮换 API token 后会清空旧复制状态；不改变 token 轮换 API、secret preview 或 AuthCenter 嵌入结构。

### 2026-07-04 U5 ShareToolbar desktop selects

- ShareToolbar 从 server 自制状态 segmented tabs 改为 desktop-style 三列控件：搜索框、状态 select、排序 select；继续保留 server 的 `for sale` 过滤项。
- 删除旧 `.share-filter-tabs` 样式，toolbar 使用响应式 grid，移动端自然落为单列；不改变 share 过滤/排序数据流。

### 2026-07-04 U9 Share modal phrase coverage

- Share ACL/Subdomain/Binding/Market/Owner/Import modal 的标题、footer label、owner handoff 步骤和 market mode 选项补齐 zh/zh-TW/ja 短语，减少 `tx()` 英文 fallback。
- Binding modal 标题改为参数化 `{{app}} Binding`，避免动态拼接标题无法翻译；不改变任何 share CRUD、binding、ACL 或 owner 验证流程。

### 2026-07-04 U9 Universal/Usage modal phrase coverage

- Universal import 结果从硬编码 `imported N universal providers` 改为参数化 `tx("imported {{count}} universal providers")`，Import modal 的 `{ providers }` 说明补齐 zh/zh-TW/ja。
- Usage Default Pricing modal 的 `N model templates` subtitle 改为 `{{count}} model templates` 参数化翻译；不改变 pricing templates、import/export 或 modal 行为。

### 2026-07-04 U9 ShareCard status phrase coverage

- ShareCard 状态区的 ACL `N users` / `private`、market `not loaded` 改为 `tx()` 输出并补齐 zh/zh-TW/ja；sale 空态复用已有 `no` 短语。
- 只改变显示文案，不改变 share ACL、market loading 或 sale/grant 状态计算。

### 2026-07-04 U5/U7 export copy feedback

- Share 和 Universal export fallback modal 在 textarea 外新增 `Copy JSON` 按钮，可在自动 clipboard 写入失败或用户需要再次复制时重试。
- 导出时和重试复制后均显示成功/失败状态，复用现有 `connect-copy-status` 样式；不改变 export API、JSON 格式或 import fallback。

### 2026-07-04 U8/U9 device flow copy feedback

- Copilot/Kiro device flow 卡片新增 `Copy code` 和 `Copy URL`，可复制 user code 与 verification URL，并显示成功/失败反馈。
- OAuth finish 的 `token request preview ready`、账号卡 fallback `account imported`、credential fallback `no credential flag`、readiness/status flags 和 quota detail summary 改为 `tx()` 短语；不改变 OAuth/device polling 或账号导入逻辑。

### 2026-07-04 U1/U2/U4/U5/U7/U8 desktop-like component paths

- 将 server web 的主入口从旧 Phase L `*Dashboard.tsx` 文件名收敛到 desktop 对应目录：`components/providers/ProviderList.tsx`、`components/share/SharePage.tsx`、`components/settings/SettingsPage.tsx`、`components/settings/AuthCenterPanel.tsx`、`components/universal/UniversalProviderPanel.tsx`；Usage 保持 desktop 原名 `components/usage/UsageDashboard.tsx`。
- `App.tsx` 改为从上述 desktop-like 路径导入视图组件；`lib/api.ts` 中对应数据 loader/type 命名同步为 `loadProviderListData`、`loadSharePageData`、`loadSettingsPageData`、`SettingsPageData`、`loadAuthCenterPanelData`。
- 本切片不改变渲染结构、REST/web-runtime API、schema 或业务动作，只清理旧 Dashboard 入口命名，避免后续迁移继续围绕 Phase L 文件结构扩写。

### 2026-07-04 U0/U2 provider component topology

- 新增 `components/providers/index.ts`、`components/share/index.ts`、`components/settings/index.ts`、`components/universal/index.ts`、`components/usage/index.ts` barrel，`App.tsx` 改为通过子目录入口导入视图组件，靠近 desktop 的组件导出形态。
- 将 `ProviderEmptyState` 从 `ProviderList.tsx` 抽为 `components/providers/ProviderEmptyState.tsx`，并把通用 `appLabel()` 移到 `components/providers/providerDisplay.ts`，为后续继续拆 `ProviderCard` / `ProviderActions` 留出边界。
- 不改变 provider 空态 DOM、样式 class、文案、导入/创建动作或任何 API；本切片是纯组件拓扑整理。

### 2026-07-05 U2 ProviderListToolbar split

- 将 `ProviderListToolbar` 从 `ProviderList.tsx` 抽为 `components/providers/ProviderListToolbar.tsx`，并从 `components/providers/index.ts` 导出，进一步对齐 desktop provider 目录的列表/子组件边界。
- 保留现有 search input、visible/total 计数、CSS class、`tx("Search providers")` 和 `tx("{{visible}}/{{total}} providers")` 文案；不改变 provider filter 数据流、排序、DND 或 API。

### 2026-07-05 U2 provider badge component split

- 将 `ProviderHealthIndicator` 和 `FailoverPriorityBadge` 从 `ProviderList.tsx` 抽到 `components/providers/ProviderHealthIndicator.tsx` 与 `components/providers/FailoverPriorityBadge.tsx`，并从 provider barrel 导出。
- 保留现有 health status/latency 计算、primary/fallback label、CSS class 和 i18n 文案；不改变 ProviderCard DOM 层级以外的业务行为、failover queue、breaker reset 或 health 数据来源。

### 2026-07-05 U2 Provider action primitive

- 新增共享 `components/IconAction.tsx`，Provider 侧 hover action 按钮改为复用该组件，为后续继续抽 `ProviderActions` / 复用 Share action 铺路。
- 保留 Provider 侧现有 wrapper、disabled tooltip、busy spinner、danger class、aria-label 和 `tx()` 文案逻辑；Share 侧暂未切换，避免改变其无 wrapper 的按钮结构。

### 2026-07-05 U5 Share action primitive reuse

- `IconAction` 增加 `wrap={false}` 模式，ShareCard actions 改为复用共享组件，同时保留 Share 原有无 wrapper button DOM、title、aria-label、busy spinner 和 disabled/danger 行为。
- 该切片不改变 Share pause/resume、start/stop tunnel、connect-info、reset usage、market authorize 或 delete 的 action key / API 调用。

### 2026-07-05 U4/U7 shared action primitive reuse

- Universal provider cards 与 AuthCenter account cards 改为复用共享 `IconAction` 的 `wrap={false}` 模式，删除两个页面本地重复 action helper。
- 保留 Universal Sync/Duplicate/Edit/Delete 与 Auth refresh/quota/plan/delete 的 busy/disabled/danger/title/aria 行为；不改变任何 API 调用或 confirm 流程。

### 2026-07-05 shared StatusPill

- 新增共享 `components/StatusPill.tsx`，Provider/Share/Universal/Settings/AuthCenter/Usage 全部复用同一个 status pill 组件，删除 6 个页面内重复实现，并清理 App 中未使用的本地 `StatusPill`。
- 保留原有 `.status-pill success|warning|danger` class、children 渲染和 tone 取值；不改变任何状态判定、文案、过滤或 API。

### 2026-07-05 shared KeyValue

- 新增共享 `components/KeyValue.tsx`，Provider/Share/Universal/Settings/AuthCenter/Usage 全部复用同一个 compact key/value 组件，删除 6 个页面内重复实现。
- 共享组件内部继续使用 `tx(label)`，保留原有 label 翻译、`.compact-kv` DOM/class 和 value 渲染；不改变任何数据来源、格式化函数或 API。

### 2026-07-05 shared SimpleModal

- 新增共享 `components/SimpleModal.tsx`，合并 Universal/Share/Usage 三处重复 `SimpleModal` 实现；共享版本同时支持 `titleVariables` 和 `subtitleVariables`，覆盖三个变体的全部调用点。
- Usage 本地 `SimpleModal` 使用 `X size={15}`，共享版本统一为 `X size={16}`，1px 差异在桌面端不可感知，且与 Provider form modal 的 close 按钮尺寸一致。
- 不改变任何 modal 的 DOM 结构、CSS class、title/subtitle 翻译路径或 onClose 行为。

### 2026-07-05 shared LoadingBlock

- 新增共享 `components/LoadingBlock.tsx`，合并 Usage 的命名 `LoadingBlock` 组件与 Provider/Share/Settings/AuthCenter/Usage(TrendPanel) 共 6 处内联 `<div className="provider-empty"><Loader2 size={22}/><span>` loading 块。
- 共享组件内部使用 `tx(label)`，保留原有 i18n 翻译路径和 `.provider-empty` DOM/class；Usage 的 inline-compact `Loader2 size={18}` loading（DataSourceBar 内）保持原样，因为它是不同视觉密度的 inline variant。
- 不改变任何数据来源、loading 状态判定或 API。

### 2026-07-05 shared TextField

- 新增共享 `components/TextField.tsx`，合并 Universal（带 `disabled`）和 Settings（不带 `disabled`）两处重复 `TextField` 实现；共享版本以 `disabled?` optional 参数兼容两种调用。
- Settings 调用点继续传入 `label={t(...)}`（预翻译 key），共享组件内部 `tx(label)` 对已翻译文本做 no-op，行为与原 Settings 本地 TextField 完全一致。
- 不改变任何表单 DOM、input 行为、onChange 数据流或 provider/router/tunnel/email 保存路径。

### 2026-07-05 Universal/Share empty state component extraction

- 将 `UniversalEmptyState` 从 `UniversalProviderPanel.tsx` 抽到 `components/universal/UniversalEmptyState.tsx`，并从 `components/universal/index.ts` 导出。
- 将 `ShareEmptyState` 从 `SharePage.tsx` 抽到 `components/share/ShareEmptyState.tsx`，并从 `components/share/index.ts` 导出。
- 保留原有 DOM、CSS class、i18n 文案、props 和 import/create/preset 动作；不改变任何 API、过滤或数据流。

### 2026-07-05 Universal/Share toolbar and empty state extraction

- 将 `UniversalListToolbar` 从 `UniversalProviderPanel.tsx` 抽到 `components/universal/UniversalListToolbar.tsx`，并从 `components/universal/index.ts` 导出，对齐 desktop provider 目录的列表/子组件边界。
- 将 `ShareToolbar` 从 `SharePage.tsx` 抽到 `components/share/ShareToolbar.tsx`，并导出 `ShareFilter` / `ShareSort` 类型；SharePage 改为从提取文件导入这两个类型。
- 保留现有 search/filter/sort 控件、CSS class、i18n 文案和前端过滤数据流；不改变 shares API、share CRUD 或 universal provider schema。

### 2026-07-05 shared ModalFooter

- 新增共享 `components/ModalFooter.tsx`，合并 SharePage 本地 `ModalFooter` 和 UniversalProviderPanel `ImportUniversalModal` 的内联 footer。
- 保留 cancel/submit 按钮、`modal-inline-footer` CSS class、i18n 文案和 `disabled`/`saving` 语义；不改变任何 modal 的表单提交或 API 调用路径。

### 2026-07-05 ProviderCard extraction

- 将 `ProviderCard`、`SortableProviderCard`、`ProviderCardProps`、`DragHandleProps`、`TranslateFn`、`TxFn`、`ProviderQuotaTier` 从 `ProviderList.tsx` 抽到 `components/providers/ProviderCard.tsx`，并从 provider barrel 导出 `SortableProviderCard`。
- 同步抽离 ProviderCard 子组件：`ProviderReadinessPanel`、`ReadinessFlag`、`ProviderAccountFooter`、`ProviderLimitFooter`、`LimitMetric`；辅助函数 `providerHealthSummary`、`relativeRequestTime`、`formatRequestTime`、`accountQuotaPercent`、`clampPercent`、`tierLine`、`providerExpiryLabel`、`providerCountdownLabel`、`formatRelativePast`、`formatDuration`、`normalizeTimestamp`、`formatDateTime`、`formatCompactNumber`、`formatTime`。
- 将 `asRecord`、`getString`、`env`、`setting`、`baseUrlFromProvider`、`modelFromProvider`、`apiFormatFromProvider`、`apiKeyFromProvider`、`accountSummary`、`limitLine`、`formatUsd` 统一到 `components/providers/providerDisplay.ts`，ProviderList 和 ProviderCard 均从此导入，消除循环依赖。
- ProviderList.tsx 从 2614 行降至 1991 行；不改变任何 provider DOM、CSS class、i18n 文案、API 调用或业务行为。

### 2026-07-05 AppHeader component extraction

- 将 `HeaderBuildBadge`、`HeaderShareToggle`、`HeaderProxyStatus`、`HeaderFailoverToggle` 从 `App.tsx` 抽到 `components/AppHeader.tsx`，并导出。
- 保留现有 `desktop-mini-toggle`/`desktop-build-badge` CSS class、i18n 文案、failover API 调用和 proxy status runtime shim；不改变任何 toggle 行为或 API 调用路径。
- App.tsx 从 589 行降至约 450 行；移除未使用的 `Radio`/`Shuffle` lucide import。

### 2026-07-05 providerDisplay helper consolidation

- 将 `asRecord`、`getString`、`env`、`setting`、`baseUrlFromProvider`、`modelFromProvider`、`apiFormatFromProvider`、`apiKeyFromProvider`、`accountSummary`、`limitLine`、`formatUsd` 统一到 `components/providers/providerDisplay.ts`，ProviderList 和 ProviderCard 均从此导入，消除循环依赖。
- 保留原有函数实现、返回值和调用路径；不改变 provider schema、API 或业务行为。

### 2026-07-05 ProviderCatalogModal extraction

- 将 `ProviderCatalogModal`、`filterCatalogPresets`、`filterCatalogEntries`、`entryIcon` 从 `ProviderList.tsx` 抽到 `components/providers/ProviderCatalogModal.tsx`。
- 新组件直接导入 `appLabel`、`presetIcon`、`ProviderIcon` 和 `inferIconForText`，保留原有 preset/type 搜索、recommended/A-Z 排序、结果计数、选择 preset/type 和关闭行为。
- 该切片不改变 provider catalog DOM、CSS class、i18n 文案、provider preset 创建或 provider type 选择数据流。

### 2026-07-05 ProviderFormModal extraction

- 将 `ProviderFormModal` 及其表单子组件 `ProviderAuthSection`、`ProviderModelField`、`ProviderDesktopAdvancedSection`、`ProviderJsonField` 抽到 `components/providers/ProviderFormModal.tsx`。
- 同步迁移表单专用 helper：`apiKeyUrlForProvider`、`apiFormatOptions`、`apiFormatLabel`、`uniqueStrings`、`modelOptionsFromCatalogJson`、`extractModelIds`、`modelCatalogJsonFromFetchedModels`、placeholder 生成、账号匹配、颜色 fallback、错误格式化等。
- `ProviderDraft` 由 `ProviderList.tsx` 导出类型供表单文件复用；`fetchProviderModels` 改为在表单文件中直接从 API wrapper 导入，避免运行时循环依赖。
- ProviderList.tsx 降至约 1045 行；不改变 provider add/edit form DOM、CSS class、保存 schema、fetch models 行为或任何 API 调用。

### 2026-07-05 ShareCard extraction

- 将 `ShareCard` 从 `SharePage.tsx` 抽到 `components/share/ShareCard.tsx`，并从 share barrel 导出。
- 新增 `components/share/shareDisplay.ts`，集中 `shareName`、`appLabel`、`shareBindings`、`shareUsage`、`shareUsageRatio`、`formatTime` 等 share 展示 helper，供 SharePage 和 ShareCard 共同使用。
- 为避免一次性迁移 runtime snapshot 解析造成高风险，`ShareCard` 通过 `runtimePanel` prop 保留原 `ShareRuntimePanel` 渲染位置；DOM 位置、CSS class、connect-info copy、delete/reset confirm、market/ACL/binding actions 和 API 调用路径不变。
- SharePage.tsx 降至约 1895 行；不改变 share 卡片 UI、过滤、排序、CRUD、tunnel 或 market 行为。

### 2026-07-05 UniversalCard extraction

- 将 `SortableUniversalCard`、`UniversalCard`、`UniversalCardProps`、`DragHandleProps` 从 `UniversalProviderPanel.tsx` 抽到 `components/universal/UniversalCard.tsx`。
- 同步迁移卡片专用 helper：`AppBadge`、`universalProviderIcon`、`configuredModelApps`、`redactUniversalProvider`；`enabledUniversalApps` 保留在面板内供列表搜索使用。
- 保留 Universal 卡片 DOM、CSS class、dnd 拖拽、Sync/Delete confirm、JsonPreview 配置预览、App badges 和 IconAction 行为；不改变 universal provider schema、sync/delete/duplicate/edit API 调用路径。

### 2026-07-05 Usage DataSourceBar extraction

- 将 `DataSourceBar`、`DataSourceChip`、`dataSourceIcon`、`dataSourceLabel` 从 `UsageDashboard.tsx` 抽到 `components/usage/DataSourceBar.tsx`。
- `UsageDataSourceSummary` 类型与 `emptyDataSourceSummary` 同步迁入 DataSourceBar 文件，由 UsageDashboard 导入复用；请求数/成本格式化 helper 在 DataSourceBar 内保持原逻辑。
- 保留 Data Sources 横条 DOM、CSS class、source icon mapping、loading/empty 行为、source filter 选择数据流；不改变 usage API 查询或 rollup 计算。

## 六、验证基线

- 静态：`scripts/static-checks.sh`（含 i18n 字面量扫描，U9 加入）+ `npm --prefix web-src run typecheck` + Vite build。
- 编译允许时：`cargo check --all-targets` + `cargo test`。
- UI 验收：人工 checklist（禁止 Playwright/Cypress 等自动化），以两张基线截图为对照物。
