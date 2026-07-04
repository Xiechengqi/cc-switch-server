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
18. Share：desktop `SharePage` 组件族（ShareCard/ShareStatsBar/ShareToolbar/ShareRequestLogTable/TunnelConfigPanel/Owner 对话框）vs server 自制 ShareDashboard。
19. Usage：desktop 用量视图（BarChart 入口、图表化展示）vs server 文本表 Dashboard。
20. Universal：desktop `UniversalProviderPanel` vs 自制 UniversalDashboard。
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
| U1 | App 壳移植（去侧边栏） | `src/App.tsx` header/视图切换、`AppSwitcher`、`ProxyToggle`、`FailoverToggle`、`UpdateBadge`、`useAutoCompact` | 移植顶栏布局与视图状态机；侧边栏删除；retained 视图映射：providers（默认）/share/universal/usage/settings；accounts 并入设置 AuthCenter tab（对齐 desktop IA）；excluded 图标不渲染；移除左下调试态 | 已完成：无侧边栏、desktop 风格 logo/设置/Share/Failover/app pills/工具图标/橙色+；Accounts 独立 view/header 图标已移除并迁入 Settings Auth tab；设置/备份/导入图标可跳转 Settings 对应 tab；desktop UpdateBadge 已按 server binary 形态替换为 build/version badge 并链接 About tab；useAutoCompact 为桌面窗口布局专属，server web 暂不渲染 | L | U0 | 部分完成（2026-07-04，壳 + settings tab + build badge） |
| U2 | Provider 主视图 | `providers/ProviderList|ProviderCard|ProviderEmptyState|ProviderActions|HealthStatusIndicator|FailoverPriorityBadge`、dnd 排序 | 替换 ProviderDashboard：全宽卡片列表、拖拽排序（接 sort-order shim）、当前 provider 高亮、URL/刷新时间/订阅徽章/账号 email 行内展示；统计卡片行删除；类型矩阵移出主页 | 已完成主页降噪、单列列表、desktop 风格图标/拖拽手柄视觉、URL 行、行内订阅徽章、账号/状态信息、当前 provider 高亮；真实 dnd 排序已接入并持久化 provider `sortIndex`；ProviderCard 已按当前排序展示 primary/fallback failover priority badge；完整 desktop ProviderCard 行为仍待深化 | L | U1/U10 | 部分完成（2026-07-04，ProviderCard + dnd sort + failover badge） |
| U3 | 添加/编辑对话框 | `AddProviderDialog`/`EditProviderDialog`/`providers/forms/**`（per-app fields + OAuth sections + preset picker）/`ConfirmDialog`/`JsonEditor` | 移植对话框与表单；"可用供应商类型/仅诊断"信息收进添加流程；server 专有字段（adapter readiness 等）放高级折叠 | 已完成首切：添加入口进入 provider catalog modal，presets 与 provider types 在 dialog 内选择；Provider add/edit 表单已增加图标预览、icon 名称和颜色编辑；已移植 desktop-style `ConfirmDialog` 并替换浏览器原生 confirm；已新增轻量 `JsonEditor`/`JsonPreview` 并接入 Provider/Universal/Accounts JSON 表单和各详情预览；Provider form 已新增 authentication/endpoint section，集中 manual token、managed account、base URL/API format；完整 desktop per-app form 组件复用仍待深化 | XL | U2 | 部分完成（2026-07-04，catalog + auth section + icon/JSON components） |
| U4 | 设置面板 | `settings/SettingsPage` + `LanguageSettings`/`ThemeSettings`/`GlobalProxySettings`/`ImportExportSection`/`BackupListSection`/`AuthCenterPanel`/`AboutSection` | 移植 tab 式全屏设置；Router/tunnel/upstream proxy 作为新增 tab；主题切换入口生效；现 SettingsDashboard 退役 | 已完成 SettingsDashboard 首切：General/Proxy/Router/Tunnel/Auth/Backup/ImportExport/Diagnostics/About tabs、主题切换入口、server-only tab 归位，支持 App 顶栏图标指定初始 tab；About tab 已展示 server build/version/commit metadata；ImportExport tab 已接入 providers/shares/universal JSON 导入导出；完整 desktop `SettingsPage` 组件迁移和 AuthCenter 合并仍待深化 | L | U1 | 部分完成（2026-07-04，tab 壳 + About + ImportExport） |
| U5 | Share 页面 | `share/**`（SharePage/ShareCard/ShareStatsBar/ShareToolbar/ShareRequestLogTable/TunnelConfigPanel/Owner 对话框族） | 替换 ShareDashboard；connect-info/market/grant/tunnel 等 server 能力接到 desktop 组件对应位置 | 已完成首切：Share stats bar、ShareCard 头部/状态/market badge、binding chips 接入 app/provider 图标；Owner/market/connect/tunnel 操作已保留；最近 share request log 已从宽表格改为 activity cards；新增 TunnelConfigPanel 汇总 router/tunnel/market 状态并复用 snapshot/restore/edits/markets 动作；Owner change 弹窗已改为 owner handoff 步骤面板；完整 desktop SharePage 组件复用仍待深化 | L | U1/U10 | 部分完成（2026-07-04，ShareCard/Stats/RequestLogCards/TunnelConfig/OwnerDialog） |
| U6 | Usage 页面 | desktop usage 视图（BarChart 图表、日期范围选择） | 替换 UsageDashboard 的文本表为 desktop 图表组件；保留 server 特有过滤参数 | 已完成首切：summary metric cards、轻量 SVG trend chart、range bucket 点击反填 custom range；providers/models 表格已增加 usage ranking cell 与 tokens 进度条；logs/pricing/limits 已从表格堆叠改为 desktop-style activity/pricing/limit cards；后续只剩更深的 desktop usage tab 组件复用与人工核对 | M | U1 | 部分完成（2026-07-04，图表化 + ranking + cards） |
| U7 | Universal 面板 | `universal/UniversalProviderPanel` | 替换 UniversalDashboard | 已完成首切：移除 summary tiles，Universal 卡片改为 desktop `UniversalProviderCard` 风格（品牌图标、providerType、base URL、app chips、hover actions、折叠高级预览）；Universal 卡片已接入 dnd 排序并持久化 `sortIndex`；Universal 表单已将 Claude/Codex/Gemini 配置收进 per-app section cards；完整 desktop `UniversalProviderPanel` 组件复用仍待深化 | M | U1/U10 | 部分完成（2026-07-04，卡片视觉 + dnd sort + app sections） |
| U8 | 账号/quota 组件 | `AuthCenterPanel` + `*QuotaFooter`（Claude/Codex/Gemini/Cursor/Copilot/Kiro/Ollama/Antigravity/Subscription） | AccountsDashboard 功能迁入 AuthCenter tab；quota footer 组件按 provider 渲染 | 已完成首切：AccountsDashboard 嵌入 Settings Auth tab，App 顶栏独立 accounts 入口删除；Auth tab 的账号/能力卡片已接入 provider 图标和 AuthCenter 式卡片头；ProviderCard 已按绑定 account 渲染 quota footer（plan/quota/expiry、进度条、tier 摘要）；Auth tab 新增 AuthCenter overview provider cards，汇总账号数、refresh/quota/import 能力并提供 import 入口；完整 desktop `AuthCenterPanel` 和各 provider 专属 quota footer 仍待迁移 | L | U4 | 部分完成（2026-07-04，accounts IA + AuthCenter overview + quota footer） |
| U9 | i18n 全量（承接 N2） | desktop `src/i18n/locales`（已复制） | 移植过程中每个组件保留 desktop 原 `t()` key，不再手写英文字面量；补 JSX 英文字面量扫描进 `static-checks.sh` | JSX 英文字面量静态审计已降为 0 并纳入门禁；四语言语义完整性仍待人工/词条级核对 | 贯穿 U1–U8 | U0 | 部分完成（2026-07-04，静态审计清零） |
| U10 | 品牌图标与主题 | `BrandIcons.tsx`/`ProviderIcon.tsx`/`iconInference.ts`/`ColorPicker`/`mode-toggle` | 移植图标推断与品牌图标；卡片/切换器/preset 均带图标；暗色模式端到端可用 | 已移植 server-local `ProviderIcon`、小型 desktop 图标注册表、图标推断，并接入 App 切换器/ProviderCard/preset 卡片；Provider/Universal 表单均已支持图标预览与颜色编辑；新增轻量 `ColorPicker`，替换 Provider/Universal 原生颜色字段；图标集已补 GitHub/Google Cloud/Doubao/SiliconFlow/StepFun/Meta/Huawei/NewAPI/SubRouter/ByteDance 并改为长关键词优先推断；完整 desktop 图标全集仍可继续按需补齐 | M | U0 | 部分完成（2026-07-04，品牌图标 + icon edit + ColorPicker + icon expansion） |
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
- `web-src/src/main.tsx` 已接入 `ThemeProvider` 和 `desktop-theme.css`；`desktop-theme.css` 补 `--panel/--subtle/--success/--warning/--danger` alias，保证现有 server Dashboard 在 U1 前继续可用。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U1 App 壳切片

- `web-src/src/App.tsx` 已删除左侧边栏渲染路径，改为 desktop 风格顶栏：CC Switch 品牌、设置齿轮、Share control、Failover control、Claude/Codex/Gemini segmented pills、Usage/Universal/Share/Backup/Accounts/Import/Sign out/Refresh 图标组和橙色圆形添加按钮。
- Provider app 状态提升到 App 顶栏；`ProviderDashboard` 支持受控 `activeApp`，并通过 `cc-switch-server:add-provider` 事件复用现有添加 provider 流程。
- `HeaderFailoverToggle` 直接对接 server `/api/failover` 与 `/api/failover/apps/:app`；Share control 当前进入 Share 控制面，具体 share tunnel 操作留 U5 组件迁移处理。
- 过渡边界：Provider/Share/Usage/Settings/Universal/Accounts Dashboard 内容区仍是 Phase L 自制实现；Accounts 仍保留 header 图标入口，迁入 Settings AuthCenter 留 U4/U8。
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

- `SettingsDashboard` 从单页堆叠重排为 desktop 风格 tab IA：General/Proxy/Router/Tunnel/Auth/Backup/Diagnostics；现有 server-only 能力没有删除，只按职责归入对应 tab。
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
- `SettingsDashboard` 的 Auth tab 嵌入 `AccountsDashboard`，保留既有 OAuth/device flow/quota 工具能力，同时向 desktop `AuthCenterPanel` 的归属靠拢。
- 增加 `.settings-accounts-card` 嵌入样式，隐藏嵌套 dashboard 自带 toolbar，避免 Settings 卡片内出现双重标题/边距。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U7 Universal 卡片视觉首切

- `UniversalDashboard` 移除 desktop 不存在的 summary tiles，列表直接呈现 universal provider cards。
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
- `ProviderDashboard` 按 active app 加载当前 provider id，ProviderCard 增加 `current` class 和 badge；执行 switch 后同步更新本地高亮状态。
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

- `SettingsDashboard` 支持 `initialTab`，并在外部 tab 变化时同步当前 tab。
- App 顶栏设置齿轮进入 General，备份和导入图标进入 Backup tab，减少 desktop 顶栏工具入口与 Settings 面板之间的断层。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=563873 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U8 Accounts AuthCenter 视觉深化

- `AccountsDashboard` 顶部 summary tiles 改为和 Share/desktop 设置区一致的紧凑 stats bar，避免在 Settings/Auth 内继续呈现 Phase L 管理后台风格。
- Account group、account card、capability card 均接入 `ProviderIcon` 和 `iconInference`，标题区改为 provider 图标框 + 名称/说明 + 状态 badge，更接近 desktop `AuthCenterPanel` 的 provider card 信息层级。
- Device Flow 的 GitHub Copilot/Kiro 入口切换为对应 provider 图标，减少通用手机图标造成的供应商识别差异。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U4 About tab

- `SettingsDashboard` 新增 About tab，对齐 desktop 设置页保留版本/关于入口的 IA。
- `loadSettingsDashboardData()` 接入现有 `/version`，新增 `BuildInfo` 类型；About tab 展示 server name/version/versionLine、commit id/message/time、build time、target/profile/rustc/dirty 状态。
- About tab 明确保持 server 版本定位：不引入 desktop updater、release-notes 自动检查或窗口控制，只展示 server binary build metadata 和源码 commit 链接。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U2 Provider dnd sort

- `docs/web-runtime-contract.json` 新增 `update_providers_sort_order` shim，并在 `/web-api/invoke/update_providers_sort_order` dispatcher 中落地。
- `ProviderStore` 新增 desktop-compatible `ProviderSortUpdate`，将排序写入 provider JSON 的 `sortIndex` 字段；`/api/providers` 列表按 `sortIndex` + 原始顺序返回，避免前端 reload 后丢失排序。
- `ProviderDashboard` 接入 `@dnd-kit`，Provider 卡片使用真实 sortable item；拖拽结束后乐观更新当前 app 列表，并调用 `updateProvidersSortOrder(activeApp, updates)` 持久化。
- 代价：Vite 报告主 JS chunk 超过 500 kB warning，但总 `web-dist` 体积仍低于 900 kB 门禁。
- 验证：`npm --prefix web-src run typecheck`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化，未运行编译型 cargo check。

### 2026-07-04 U4 ImportExport tab

- `SettingsDashboard` 新增 Import / Export tab，顶栏 Download/Import 图标从 Backup tab 改为打开 ImportExport tab，Backup tab 回到只负责 server 备份快照。
- 新增 provider import/export API wrapper，复用已有 `/api/providers/export` 与 `/api/providers/import`；providers 导入时按 server API 要求只提交 `{ app, provider }`。
- ImportExport tab 提供 providers、shares、universal providers 三组 JSON 面板，支持一键导出到 textarea 和粘贴 JSON 导入；Universal 导出使用既有 `{ providers: [...] }` 格式，同时导入兼容数组、`{ providers }` 与 `{ universal }`。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；未启动 server，未做 UI 自动化。

### 2026-07-04 U3/U10 Provider icon edit

- `ProviderDraft` 新增 `icon` 与 `iconColor` 字段，create/edit 均可编辑；编辑时读取 provider 顶层 `icon`/`iconColor`，保存时写回 provider JSON。
- `ProviderFormModal` 复用 Universal 表单的图标编辑形态，展示 `ProviderIcon` 预览、icon 名称输入和原生 color input；未手动指定 icon 时继续使用 `iconInference` 推断预览。
- 该字段已被现有 `storedProviderIcon()` 消费，因此保存后 ProviderCard、Share binding 等使用 provider icon 的位置会自动显示自定义图标。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=620873 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U5 Share request logs

- `loadShareDashboardData()` 现在同步加载最近 usage logs，并在 Share 页面按现有 share id 过滤为 share request logs；不新增后端 API。
- Share 页面新增 `ShareRequestLogPanel`，以 desktop request log table 风格展示最近 share 请求的 share/app/model/status/tokens/cost/latency/user/time。
- 现有 Owner/market/connect/tunnel/import/export/share card 行为未改动；日志表只读，作为 SharePage 组件族对齐切片。
- 验证：`npm --prefix web-src run typecheck`、`cargo fmt -- --check`、`npm --prefix web-src run build`、`scripts/static-checks.sh` 均通过；`webDistBytes=623414 < 900000`；未启动 server，未做 UI 自动化。

### 2026-07-04 U7 Universal dnd sort

- `UniversalDashboard` 接入 `@dnd-kit`，Universal provider 卡片使用真实 sortable item；拖拽手柄复用 ProviderCard 的 `GripVertical` 视觉和可访问 label。
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

- `AccountsDashboard` 新增 `AuthCenterOverview`，放在 Settings/Auth 的账号统计之后，按 provider type 展示 Auth Center provider cards。
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

## 六、验证基线

- 静态：`scripts/static-checks.sh`（含 i18n 字面量扫描，U9 加入）+ `npm --prefix web-src run typecheck` + Vite build。
- 编译允许时：`cargo check --all-targets` + `cargo test`。
- UI 验收：人工 checklist（禁止 Playwright/Cypress 等自动化），以两张基线截图为对照物。
