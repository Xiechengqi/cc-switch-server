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
| U1 | App 壳移植（去侧边栏） | `src/App.tsx` header/视图切换、`AppSwitcher`、`ProxyToggle`、`FailoverToggle`、`UpdateBadge`、`useAutoCompact` | 移植顶栏布局与视图状态机；侧边栏删除；retained 视图映射：providers（默认）/share/universal/usage/settings；accounts 并入设置 AuthCenter tab（对齐 desktop IA）；excluded 图标不渲染；移除左下调试态 | 已完成：无侧边栏、desktop 风格 logo/设置/Share/Failover/app pills/工具图标/橙色+；Accounts 独立 view/header 图标已移除并迁入 Settings Auth tab；设置/备份/导入图标可跳转 Settings 对应 tab；UpdateBadge/useAutoCompact 等桌面专属入口仍待按 server 适配 | L | U0 | 部分完成（2026-07-04，壳 + settings tab 跳转） |
| U2 | Provider 主视图 | `providers/ProviderList|ProviderCard|ProviderEmptyState|ProviderActions|HealthStatusIndicator|FailoverPriorityBadge`、dnd 排序 | 替换 ProviderDashboard：全宽卡片列表、拖拽排序（接 sort-order shim）、当前 provider 高亮、URL/刷新时间/订阅徽章/账号 email 行内展示；统计卡片行删除；类型矩阵移出主页 | 已完成主页降噪、单列列表、desktop 风格图标/拖拽手柄视觉、URL 行、行内订阅徽章、账号/状态信息、当前 provider 高亮；真实 dnd 排序和完整 desktop ProviderCard 行为仍待深化 | L | U1/U10 | 部分完成（2026-07-04，ProviderCard 当前高亮） |
| U3 | 添加/编辑对话框 | `AddProviderDialog`/`EditProviderDialog`/`providers/forms/**`（per-app fields + OAuth sections + preset picker）/`ConfirmDialog`/`JsonEditor` | 移植对话框与表单；"可用供应商类型/仅诊断"信息收进添加流程；server 专有字段（adapter readiness 等）放高级折叠 | 已完成首切：添加入口进入 provider catalog modal，presets 与 provider types 在 dialog 内选择；完整 desktop per-app 表单/OAuth sections/ConfirmDialog/JsonEditor 仍待深化 | XL | U2 | 部分完成（2026-07-04，catalog 首切） |
| U4 | 设置面板 | `settings/SettingsPage` + `LanguageSettings`/`ThemeSettings`/`GlobalProxySettings`/`ImportExportSection`/`BackupListSection`/`AuthCenterPanel`/`AboutSection` | 移植 tab 式全屏设置；Router/tunnel/upstream proxy 作为新增 tab；主题切换入口生效；现 SettingsDashboard 退役 | 已完成 SettingsDashboard 首切：General/Proxy/Router/Tunnel/Auth/Backup/Diagnostics tabs、主题切换入口、server-only tab 归位，支持 App 顶栏图标指定初始 tab；完整 desktop `SettingsPage` 组件迁移、AuthCenter 合并和 About/ImportExport 仍待深化 | L | U1 | 部分完成（2026-07-04，tab 壳与顶栏跳转） |
| U5 | Share 页面 | `share/**`（SharePage/ShareCard/ShareStatsBar/ShareToolbar/ShareRequestLogTable/TunnelConfigPanel/Owner 对话框族） | 替换 ShareDashboard；connect-info/market/grant/tunnel 等 server 能力接到 desktop 组件对应位置 | 已完成首切：Share stats bar、ShareCard 头部/状态/market badge、binding chips 接入 app/provider 图标；完整 desktop SharePage/ShareRequestLogTable/TunnelConfigPanel/Owner 对话框仍待深化 | L | U1/U10 | 部分完成（2026-07-04，ShareCard/Stats 首切） |
| U6 | Usage 页面 | desktop usage 视图（BarChart 图表、日期范围选择） | 替换 UsageDashboard 的文本表为 desktop 图表组件；保留 server 特有过滤参数 | 已完成首切：summary metric cards、轻量 SVG trend chart、range bucket 点击反填 custom range；logs/providers/models/pricing/limits 仍保留现有 server 表格，后续再按 desktop usage tab 深化 | M | U1 | 部分完成（2026-07-04，图表化首切） |
| U7 | Universal 面板 | `universal/UniversalProviderPanel` | 替换 UniversalDashboard | 已完成首切：移除 summary tiles，Universal 卡片改为 desktop `UniversalProviderCard` 风格（品牌图标、providerType、base URL、app chips、hover actions、折叠高级预览）；完整 desktop form/modal 行为仍待深化 | M | U1/U10 | 部分完成（2026-07-04，卡片视觉首切） |
| U8 | 账号/quota 组件 | `AuthCenterPanel` + `*QuotaFooter`（Claude/Codex/Gemini/Cursor/Copilot/Kiro/Ollama/Antigravity/Subscription） | AccountsDashboard 功能迁入 AuthCenter tab；quota footer 组件按 provider 渲染 | 已完成首切：AccountsDashboard 嵌入 Settings Auth tab，App 顶栏独立 accounts 入口删除；完整 desktop `AuthCenterPanel` 和各 quota footer 组件仍待迁移 | L | U4 | 部分完成（2026-07-04，accounts IA 首切） |
| U9 | i18n 全量（承接 N2） | desktop `src/i18n/locales`（已复制） | 移植过程中每个组件保留 desktop 原 `t()` key，不再手写英文字面量；补 JSX 英文字面量扫描进 `static-checks.sh` | JSX 英文字面量静态审计已降为 0 并纳入门禁；四语言语义完整性仍待人工/词条级核对 | 贯穿 U1–U8 | U0 | 部分完成（2026-07-04，静态审计清零） |
| U10 | 品牌图标与主题 | `BrandIcons.tsx`/`ProviderIcon.tsx`/`iconInference.ts`/`ColorPicker`/`mode-toggle` | 移植图标推断与品牌图标；卡片/切换器/preset 均带图标；暗色模式端到端可用 | 已移植 server-local `ProviderIcon`、小型 desktop 图标注册表、图标推断，并接入 App 切换器/ProviderCard/preset 卡片；完整图标全集、ColorPicker、主题切换 UI 仍待 U4/U10 后续 | M | U0 | 部分完成（2026-07-04，品牌图标基础） |
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

## 六、验证基线

- 静态：`scripts/static-checks.sh`（含 i18n 字面量扫描，U9 加入）+ `npm --prefix web-src run typecheck` + Vite build。
- 编译允许时：`cargo check --all-targets` + `cargo test`。
- UI 验收：人工 checklist（禁止 Playwright/Cypress 等自动化），以两张基线截图为对照物。
