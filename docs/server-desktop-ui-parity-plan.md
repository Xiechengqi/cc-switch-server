# Server 前端 UI 对齐 Desktop 完整实施计划

> **产品目标**：desktop 用户迁移到 server 时 **无感**——每个 retained 页面的按钮、文字、大小、样式、交互路径与 desktop 一致。  
> **技术策略**：**同源移植** desktop React 组件，仅在 runtime 边界适配；禁止继续扩写 server-local 等价 Dashboard。  
> **关联文档**：`assets/contract/web-runtime-contract.json`（功能边界）、`docs/manual-ui-checklist.md`（人工验收门禁）。本地-only 历史笔记（不提交）：`UI_PARITY_PLAN.md`、`DESKTOP_ALIGNMENT_TASKS.md`、`docs/remaining-work-index.md`。

**文档版本**：2026-07-05（2026-07-21 更新：Universal Providers、桌面自动故障转移、全局出站代理与通用配置导入导出已从 token server 移除，见 `web-runtime-contract.json` `excludedFeatures`）
**Desktop 基线仓库**：`/data/projects/cc-switch`  
**Server 前端目录**：`/data/projects/cc-switch-server/web-src/`

> **2026-07-10 范围收缩**：下列计划中涉及 Universal Providers、Settings/universal tab、`components/universal/**`、`universalProviderPresets.ts`、以及 Provider「导入当前配置」的条目均已 **作废**；server 以 per-app `providers.json` 为唯一配置源，历史 Universal 数据只进入显式迁移预检，普通启动不写盘。

> **2026-07-22 Server 边界**：server 必须搭配 Router；Share、显式 Provider 和所有非 Claude 请求固定使用选定 binding，同一请求不得切换 Provider。仅未固定的直接 Claude Messages/count_tokens 请求可在受控失败、3 次/10s 总预算和下游 commit 前按 Provider Store 顺序 failover。桌面可配置的自动故障转移、全局出站代理和 Settings 数据管理导入导出仍为 excluded。跨环境迁移使用停机复制完整数据目录，见 `docs/server-data-migration.md`。

> **2026-07-21 Provider 边界**：Provider 业务表单不再追求 desktop 源码同构。Server 使用 Rust registry 投影和 `server/providers/editor/ServerProviderForm.tsx`，只呈现有真实 Server consumer 的 typed 字段。`components/providers/forms/ProviderForm.tsx` 及其 OpenCode/OpenClaw/Hermes/Claude Desktop 分支仅作为 pinned desktop 同步参考，Server Add/Edit 不导入它们；`audit-server-product-boundary.mjs` 固化该入口约束。下文 U12.2/U12.7 中与此冲突的旧任务均以本条为准。

---

## 一、已确认的产品决策（2026-07-05）

| 决策项 | 结论 | 影响 |
| --- | --- | --- |
| Embedded 体积预算 | **接受总量从 900KB 提升至约 4.5MB**（`CC_SWITCH_WEB_DIST_MAX_BYTES=4503592`） | 可引入 CodeMirror `JsonEditor`、recharts `UsageDashboard`、完整四语言 locale chunk；当前实测 ~4.30MB，余量约 200KB（4.8%） |
| Server 扩展能力归属 | **全部放入 Settings**，不单独占顶栏一级视图 | Usage / Router / Tunnel / Diagnostics 均在 Settings tab 内 |
| 对齐粒度 | **逐组件 pixel + 行为 + 文案 parity** | 验收标准高于 Phase U「风格等价实现」 |
| UI 自动化 | **禁止** Playwright/Cypress 等 | 沿用 `docs/manual-ui-checklist.md` 人工核对 |

---

## 二、范围边界

### 2.1 Retained（必须对齐）

依据 `assets/contract/web-runtime-contract.json`：

- Providers（Claude Code / Codex / Gemini）
- Shares
- Usage & Pricing
- Settings（含 Auth、Router、Backup、Tunnel、Diagnostics）
- Accounts / OAuth / Quota（归属 Settings → Auth）
- Proxy/Provider 路由状态（Settings 中只读展示）

### 2.2 Excluded（不得出现在导航或主操作）

- MCP、Skills、Prompts（hidden）、Session Manager（hidden）
- OpenClaw / Hermes / OMO / Claude Desktop profile
- Universal Providers、桌面自动故障转移、全局出站代理、通用配置导入导出
- Tauri shell（托盘、更新器、deeplink、窗口控制）
- WebDAV/S3 云同步、Speedtest、本地 CLI session 解析
- `codex_responses_ws`

### 2.3 App 范围

Server 仅保留 **claude / codex / gemini** 三个 App pill；desktop 其余 App Tab 不渲染。

---

## 三、现状评估

### 3.1 Phase U（U0–U10）已完成的工作

Phase U 将 UI 从 Phase L「自创管理后台」拉回 **desktop 风格等价实现**：

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| U0 | shadcn/Radix 基座、ThemeProvider、tailwind token | 已完成 |
| U1 | 去侧边栏、desktop 顶栏壳、橙色 + | 代码收口，待 U11 |
| U2 | Provider 列表降噪、dnd、普通 health badge | 代码收口，待 U11 |
| U3 | Catalog modal、轻量表单、轻量 JsonEditor | 代码收口，待 U11 |
| U4–U8 | Settings/Share/Usage/Auth 首切 | 代码收口，待 U11；Universal 已取消 |
| U9–U10 | i18n 静态门禁、ProviderIcon、主题 | 代码收口，待 U11 |
| U11 | 人工逐像素核对 | **待办** |

### 3.2 Phase U 与「无感迁移」的差距

| 维度 | Desktop | Server（Phase U 后） | 差距 |
| --- | --- | --- | --- |
| 组件文件数 | ~231 | ~105 | 未移植 ~56% |
| Provider 表单 | desktop `providers/forms/**` | Server-native registry 驱动 typed editor | **有意分叉；核心入口已收口** |
| 样式源 | `index.css` + shadcn utility | `styles.css` ~5700 行自定义 class | **双设计系统** |
| Quota 展示 | 9 个专属 `*QuotaFooter` | 内联通用 meter | 视觉/文案不等同 |
| Json 编辑 | CodeMirror | textarea | 交互不等同 |
| Usage 图表 | recharts | 轻量 SVG | 视觉不等同 |
| i18n | `react-i18next` + 完整 `t()` key | `tx()` 短语表 + 部分 `t()` | 词条未完全同源 |
| 动画 | framer-motion | 未接入源码 | 过渡不一致 |

**结论**：Phase U 解决了「看起来像同一个产品家族」，**不足以**实现 desktop 用户无感迁移。需要 **Phase U12 同源移植**。

### 3.3 Phase U12.0 地基（2026-07-05 已落地）

| 项 | 变更 |
| --- | --- |
| 体积门禁 | `scripts/audit/audit-web-dist-size.mjs` 默认上限 **4.5MB**（`4503592`） |
| Vite 分包 | `codemirror` / `recharts` / `framer-motion` / `locales` 独立 chunk |
| 运行时基座 | `QueryClientProvider` + `Toaster` + desktop `index.css` |
| i18n | `web-src/src/i18n/index.ts` 加载完整四语言 locale；`t()` 优先 `i18n.exists` |
| 导航 IA | Usage → Settings/Usage tab；Universal 不进入 Server；顶栏仅 providers/shares/settings |
| 同步工具 | `scripts/sync/sync-desktop-ui.mjs` |

当前构建体积约 **4.30MB**（`web-dist` 合计 4,297,314 B；余量约 206KB / 4.8%）。`locales`、`codemirror`、`recharts` 已独立 chunk；下次 desktop sync 或品牌图标增补前需评估体积影响。

---

## 四、对齐原则

1. **移植而非重写**：以 `/data/projects/cc-switch/src/components/**` 为唯一 UI 基线，路径镜像到 `web-src/src/components/**`。
2. **只改 runtime 边界**：`invokeCommand` → `/web-api/invoke/<command>`（90 命令 registry 已就绪）；`isTauriRuntime()` 为 false 时走 web 分支。
3. **Tauri 专属 API 用 shim 替代**：`openExternal` → `window.open`；`pick_directory` / `save_file_dialog` → 隐藏或 HTML fallback；`update_tray_menu` / `check_for_updates` → no-op。
4. **Server 扩展能力**：以 **desktop Settings tab 样式** 新增 tab，不引入第二套 UI 语言。
5. **验收**：人工 checklist + 截图对比；禁止 UI 自动化。

---

## 五、信息架构（IA）对照

### 5.1 顶栏视图（对齐后）

| Desktop | Server（U12 目标） |
| --- | --- |
| `providers`（默认） | `providers` |
| `shares`（Claude/Codex/Gemini 上下文） | `shares` |
| `settings`（全屏面板） | `settings` |
| Usage（顶栏图标 → Settings/usage tab） | 同左 |
| Universal（AddProviderDialog tab，无顶栏入口） | excluded；只保留迁移 inventory，不渲染入口 |
| MCP/Skills/Sessions/… | 不渲染 |

### 5.2 Settings Tab 结构（对齐后）

**Desktop 基线（6 tab）**：general · proxy · auth · advanced · usage · about

**Server 扩展 tab（同风格并入）**：

| Tab | 来源 | 说明 |
| --- | --- | --- |
| general | desktop | 语言/主题/App 可见性等；server 补 runtime 摘要 |
| language | server | 可合并进 general（移植 desktop `LanguageSettings` 后评估） |
| theme | server | 可合并进 general（移植 desktop `ThemeSettings` 后评估） |
| directory | server | 配置目录、web-dist、embedded assets 路径 |
| proxy | server | 只读展示监听地址、运行状态和 Claude/Codex/Gemini 当前 Provider |
| router | server | router 注册、heartbeat、batch sync |
| tunnel | server | client tunnel claim/start/stop |
| auth | desktop | `AuthCenterPanel` + 账号/OAuth/quota |
| backup | desktop | 快照列表、创建/恢复 |
| usage | desktop | 嵌入 desktop `UsageDashboard` |
| universal | excluded | 不渲染；Universal 仅为迁移输入 |
| diagnostics | server | tunnel/share sync、健康摘要 |
| about | desktop | build/version/commit；**无** Tauri updater |

移植 desktop `SettingsPage` 后，将 server-only tab 作为 **同组件族的新 TabsTrigger**，tab 数量可多于 desktop 6 个，但 **单个 tab 内视觉必须与 desktop 一致**。

---

## 六、Phase U12 任务分解

### 总览

```
U12.0 地基（已完成）
    ↓
U12.1 设计系统统一 ──→ U12.2 Provider 栈（关键路径）
    ↓                        ↓
U12.3 App 壳原样移植    U12.4 Settings 原样移植 + server tab
    ↓                        ↓
U12.5 Share / U12.6 Usage（并行；U12.7 已取消）
    ↓
U12.8 Quota Footer + 共享组件收尾
    ↓
U12.9 i18n 全量切 t() key
    ↓
U11 / U12.10 人工验收收口
```

**关键路径**：`U12.1 → U12.2 → U12.3`（Provider 表单占用户感知 ~50%）。

---

### U12.0 地基 ✅ 已完成

| 验收项 | 命令 |
| --- | --- |
| typecheck | `npm --prefix web-src run typecheck` |
| build | `npm --prefix web-src run build` |
| 体积 | `node scripts/audit/audit-web-dist-size.mjs`（< 4.5MB） |
| 静态门禁 | `scripts/static-checks.sh` |

---

### U12.1 设计系统统一

**目标**：废除 server 自建 `styles.css` 对 retained 页面的样式支配，全面使用 desktop `index.css` + shadcn component class。

| ID | 任务 | Desktop 来源 | Server 目标 | 验收标准 | 工作量 |
| --- | --- | --- | --- | --- | --- |
| U12.1a | 锁定样式源 | `src/index.css` | `web-src/src/index.css`（已复制，随 sync 更新） | `main.tsx` 仅引入 `index.css` + 过渡期 `styles.css`；`components.json` 指向 `index.css` | S |
| U12.1b | 删除过渡 CSS | — | 逐页删除 `styles.css` 中已移植组件用到的 class | 每完成一个 U12.2–U12.7 切片，删除对应 CSS block；最终 `styles.css` 归零或仅留 server-login 最小集 | L |
| U12.1c | framer-motion 接入 | desktop 各面板 transition | 移植组件时保留 `motion.div` | Settings/Dialog 切换动画与 desktop 一致 | M |
| U12.1d | 图标注册表同步 | `src/icons/extracted/**` | `web-src/src/icons/extracted/**` | `ProviderIcon` / preset 卡片图标集与 desktop 一致（MB 级 SVG 按体积豁免登记） | M |

**依赖**：U12.0  
**阻塞**：U12.2–U12.7 所有视觉 parity

---

### U12.2 Provider 栈（关键路径）

**目标**：Provider 主视图 + 添加/编辑对话框与 desktop **同源组件**。

#### 同步清单（`scripts/sync/sync-desktop-ui.mjs`）

```
components/providers/ProviderList.tsx
components/providers/ProviderCard.tsx
components/providers/ProviderActions.tsx
components/providers/ProviderEmptyState.tsx
components/providers/ProviderHealthIndicator.tsx
components/providers/AddProviderDialog.tsx
components/providers/EditProviderDialog.tsx
components/providers/ProviderPresetSelector.tsx
components/providers/forms/**          # pinned desktop 视觉/兼容参考，不是 Server 表单入口
server/providers/editor/**              # Server registry 驱动的生产表单
server/providers/useServerProviderActions.ts
components/ClaudeOauthQuotaFooter.tsx
components/CodexOauthQuotaFooter.tsx
components/GeminiOauthQuotaFooter.tsx
components/CopilotQuotaFooter.tsx
components/CursorOauthQuotaFooter.tsx
components/KiroOauthQuotaFooter.tsx
components/AntigravityOauthQuotaFooter.tsx
components/OllamaQuotaFooter.tsx
components/SubscriptionQuotaFooter.tsx
components/ConfirmDialog.tsx
components/JsonEditor.tsx              # CodeMirror，lazy chunk
components/ProviderIcon.tsx
components/BrandIcons.tsx
config/claudeProviderPresets.ts
config/codexProviderPresets.ts
config/geminiProviderPresets.ts
config/universalProviderPresets.ts
config/iconInference.ts
hooks/useProviderActions.ts             # desktop compatibility，Server 入口不导入
lib/query/**                           # mutations 随组件需求
```

#### 任务表

| ID | 任务 | 验收标准 | 工作量 |
| --- | --- | --- | --- |
| U12.2a | 固定 registry/editor contract | `npm run typecheck` 与 Provider registry audit 通过 | L |
| U12.2b | Server runtime boundary | Add/Edit 直接导入 `ServerProviderForm`；不经过 desktop dispatcher | M |
| U12.2c | ProviderList/Card 接线 | 列表、dnd、当前 provider 高亮、hover actions、9 种 QuotaFooter 按类型渲染 | L |
| U12.2d | Add/Edit Dialog 接线 | Profile、credential、account、model、driver section 与 Rust registry 一致 | XL |
| U12.2e | 隔离 desktop editor | raw env/TOML/auth、OpenCode/OpenClaw/Hermes/Claude Desktop 不进入 production graph | M |
| U12.2f | Server 诊断字段 | readiness、identity migration、storage migration 只读展示，不污染 draft | M |

**依赖**：U12.1a  
**风险**：desktop sync 可能重新引入 dispatcher；由 `audit-server-product-boundary.mjs` 阻断。

---

### U12.3 App 壳原样移植

**目标**：顶栏每个按钮的位置、尺寸、icon size、tooltip 文案与 desktop 一致。

| ID | 任务 | Desktop 来源 | 差异处理 | 验收标准 | 工作量 |
| --- | --- | --- | --- | --- | --- |
| U12.3a | 移植 header 区块 | `App.tsx` header ~1779–1950 行 | 剔除 excluded 视图按钮、窗口控制、`useAutoCompact` | 顶栏布局 pixel 对齐 | L |
| U12.3b | ProxyToggle | `components/proxy/ProxyToggle.tsx` | `readOnly` prop：server 无 live takeover 写操作 | 视觉一致；点击跳转 Settings/proxy | M |
| U12.3c | FailoverToggle | excluded | Server 不渲染自动故障转移入口 | 已删除 | — |
| U12.3d | UpdateBadge → BuildBadge | `UpdateBadge.tsx` | 显示 build/commit；不调用 updater | 同位置同尺寸 | S |
| U12.3e | Usage 入口 | 顶栏 BarChart2 | `openSettings("usage")` | 与 desktop 路径一致 | S |
| U12.3f | 删除 server App.tsx 手写顶栏 | — | 使用移植后 desktop 壳 | `App.tsx` < 600 行（不含 login 分支） | M |

**依赖**：U12.2c（Provider 为默认视图）

---

### U12.4 Settings 原样移植 + Server Tab

| ID | 任务 | 验收标准 | 工作量 |
| --- | --- | --- | --- |
| U12.4a | 移植 desktop `SettingsPage.tsx` | general/proxy/auth/advanced/usage/about 六 tab 与 desktop 一致 | L |
| U12.4b | 并入 server tab | router/tunnel/directory/diagnostics 用相同 `TabsTrigger`/`TabsContent` 样式 | M |
| U12.4c | 移植子面板 | `LanguageSettings`、`ThemeSettings`、`BackupListSection`、`AuthCenterPanel`、`AboutSection` | L |
| U12.4d | 嵌入 Usage | Settings/usage → desktop `UsageDashboard`；Universal 保持 excluded | M |
| U12.4e | 删除 server-local Settings 分片 | 移除 `SettingsAccountPanels.tsx` 等过渡实现（逻辑迁入移植组件或薄 wrapper） | L |

**依赖**：U12.6（嵌入面板可先占位后替换）

---

### U12.5 Share 页面

| ID | 任务 | 来源 | 验收标准 | 工作量 |
| --- | --- | --- | --- | --- |
| U12.5a | 同步 `components/share/**` | desktop | ShareCard/Toolbar/StatsBar/RequestLog/Tunnel/Owner 对话框族同源 | L |
| U12.5b | Server API 接线 | connect-info/market/grant/tunnel | 功能不回归；UI 与 desktop 一致 | M |
| U12.5c | 删除 server-local Share 组件 | — | 无重复实现 | M |

---

### U12.6 Usage 页面

| ID | 任务 | 来源 | 验收标准 | 工作量 |
| --- | --- | --- | --- | --- |
| U12.6a | 同步 `components/usage/**` | desktop | 使用 **recharts**（已预留 chunk） | L |
| U12.6b | 日期范围选择器 | desktop `usage-range-popover` + container query | 窄屏不溢出（`index.css` 已含 container query） | M |
| U12.6c | Server 过滤参数 | advanced filters | 保留 server 特有过滤，UI 控件样式与 desktop 一致 | M |
| U12.6d | 删除 SVG 轻量图表 | — | 移除 `UsageTrendPanel` 自定义 SVG 实现 | S |

---

### U12.7 Universal 面板（已取消）

Universal 不属于 token server runtime。旧数据仅作为受控 Provider 迁移输入；不得同步面板、注册 Settings tab 或在 AddProviderDialog 中提供创建入口。

---

### U12.8 共享组件与 Auth

| ID | 任务 | 来源 | 工作量 |
| --- | --- | --- | --- |
| U12.8a | `AuthCenterPanel` 原样移植 | desktop | L |
| U12.8b | `ColorPicker` / `IconPicker` / `mode-toggle` | desktop | M |
| U12.8c | `components/ui/**` 随 desktop 更新 | sync 脚本 | S |
| U12.8d | Login/Setup 面板 | 可保留 server-local，但样式对齐 desktop `ClientWebLoginPage` | M |

---

### U12.9 i18n 全量同源

| ID | 任务 | 验收标准 |
| --- | --- | --- |
| U12.9a | 淘汰 `tx()` 短语表 | retained 页面 JSX 全部使用 desktop `t("key")` |
| U12.9b | locale 随 desktop 同步 | `scripts/sync/sync-desktop-ui.mjs` 含 `i18n/locales/*.json` |
| U12.9c | 审计 | `scripts/audit/audit-web-i18n-literals.mjs` 保持 0；四语言人工审读 |

---

### U12.10 / U11 人工验收收口

依据 `docs/manual-ui-checklist.md` 与本地 `UI_PARITY_PLAN.md` 第二节 24 点（历史参考，不提交）：

| 维度 | 要求 |
| --- | --- |
| 视口 | 1366×768、~390px 窄屏 |
| 主题 | light / dark / system |
| 对比方式 | 同数据状态下 desktop vs server 截图并排 |
| 记录 | 日期、commit、viewport、失败项、跟进 task ID |

**Done 定义（无感迁移）**：

- [ ] retained 页面每个 primary button 的 variant/size/icon 与 desktop 一致
- [ ] 文案全部来自 desktop locale `t()` key
- [ ] Provider 添加/编辑流程逐步一致
- [ ] 顶栏导航路径与 desktop 一致（Usage 进 Settings）
- [ ] excluded 功能不可见
- [ ] `scripts/sync/import-desktop-export.mjs` 导入后 UI 展示一致

---

## 七、Runtime 适配层

### 7.1 不改动的边界

| 模块 | 路径 | 说明 |
| --- | --- | --- |
| Web invoke | `web-src/src/lib/runtime.ts` | `invokeCommand` → `/web-api/invoke` |
| Server API | `web-src/src/lib/api.ts` | REST 封装；移植时按需扩展 |
| Context | `GET /web-api/context` | setup/auth/apps/commands registry |

### 7.2 需要新增的 shim

建议在 `web-src/src/lib/server-shims.ts` 集中实现：

| Desktop API | Server 行为 |
| --- | --- |
| `isTauriRuntime()` | 恒 `false` |
| `openExternal(url)` | `window.open(url, "_blank", "noopener,noreferrer")` |
| `pick_directory` / `open_file_dialog` / `save_file_dialog` | 返回 cancelled 或 HTML `<input type="file">` fallback |
| `open_config_folder` | 跳转 Settings/directory 展示路径 |
| `check_for_updates` / `install_update_and_restart` | no-op；About 显示 build info |
| `update_tray_menu` | no-op |
| `set_window_theme` | 已由 `ThemeProvider` 处理 |

### 7.3 invoke 契约

所有 retained UI 调用的 command 必须在 `assets/contract/web-runtime-contract.json` 登记，并通过：

```bash
node scripts/audit/audit-web-runtime-contract.mjs --check
```

---

## 八、构建与体积策略

### 8.1 体积门禁

| 项 | 值 |
| --- | --- |
| 默认上限 | **4.5MB**（`CC_SWITCH_WEB_DIST_MAX_BYTES=4503592`） |
| 当前基线 | ~4.30MB（4,297,314 B，余量约 206KB / 4.8%） |
| 审计脚本 | `scripts/audit/audit-web-dist-size.mjs` |

### 8.2 Vite 分包（`web-src/vite.config.ts`）

| Chunk | 内容 | 加载策略 |
| --- | --- | --- |
| `locales` | 四语言 JSON | 首屏（已独立） |
| `codemirror` | JsonEditor | `import()` 懒加载 |
| `recharts` | UsageDashboard | 进入 Settings/usage 时懒加载 |
| `framer-motion` | 动画 | 随移植组件按需 |

### 8.3 构建链路

```
web-src/  --npm run build-->  web-dist/  --build.rs-->  embedded binary
```

本地调试覆盖：`--web-dist-dir /path/to/web-dist`

### 8.4 品牌图标豁免登记

以下 desktop `src/icons/extracted/` 资产未同步到 server `web-src/src/icons/extracted/`。除 `mcp.svg` 属 excluded MCP 功能资产外，其余均为长尾供应商品牌位图/专属标志；当前由 `iconInference` 回退显示。为维持 embedded web asset 体积门禁，暂不纳入首包。登记基于 2026-07-07 实测，缺失资产合计约 565KB。

| 文件 | 大小 | 处理 |
| --- | ---: | --- |
| `ClaudeApi.png` | 17,658 B | 品牌位图，体积豁免 |
| `TeamoRouter-icon-dark.png` | 1,744 B | 品牌位图，体积豁免 |
| `amuxapi-icon.svg` | 329 B | 长尾品牌 SVG，随懒加载图标方案再评估 |
| `apikeyfun.png` | 1,094 B | 品牌位图，体积豁免 |
| `apinebula_icon.png` | 6,632 B | 品牌位图，体积豁免 |
| `atlascloud_icon.png` | 9,604 B | 品牌位图，体积豁免 |
| `byteplus.png` | 27,347 B | 品牌位图，体积豁免 |
| `cherryin.png` | 11,327 B | 品牌位图，体积豁免 |
| `claudecn.png` | 47,341 B | 品牌位图，体积豁免 |
| `code0.png` | 2,323 B | 品牌位图，体积豁免 |
| `eflowcode.png` | 63,679 B | 品牌位图，体积豁免 |
| `etok.png` | 42,576 B | 品牌位图，体积豁免 |
| `fenno-icon.webp` | 33,198 B | 品牌位图，体积豁免 |
| `huoshan.png` | 35,396 B | 品牌位图，体积豁免 |
| `mcp.svg` | 978 B | MCP excluded 功能资产，永久跳过 |
| `nekocode-icon.png` | 2,852 B | 品牌位图，体积豁免 |
| `pateway.jpg` | 7,283 B | 品牌位图，体积豁免 |
| `pipellm.png` | 1,969 B | 品牌位图，体积豁免 |
| `qiniu.png` | 70,218 B | 品牌位图，体积豁免 |
| `relaxcode.png` | 41,716 B | 品牌位图，体积豁免 |
| `runapi.jpg` | 9,544 B | 品牌位图，体积豁免 |
| `shengsuanyun.svg` | 52,411 B | 长尾品牌 SVG，体积豁免 |
| `sudocode.png` | 37,023 B | 品牌位图，体积豁免 |
| `zetaapi-icon.png` | 40,569 B | 品牌位图，体积豁免 |

---

## 九、同步与漂移控制

### 9.1 同步脚本

```bash
# 全量同步默认路径
node scripts/sync/sync-desktop-ui.mjs

# 仅同步 desktop Provider 参考组件；不会改变 Server 生产表单入口
node scripts/sync/sync-desktop-ui.mjs components/providers/forms

# 检查 server 是否落后于 desktop
node scripts/sync/sync-desktop-ui.mjs --check
```

默认同步路径见 `scripts/sync/sync-desktop-ui.mjs` 内 `defaultPaths`。

### 9.2 Desktop 上游吸收流程

每次 desktop UI 有提交时：

1. 运行 `node scripts/sync/sync-desktop-ui.mjs --check`
2. 更新 `UPSTREAM_IMPORT.md`
3. 按需 `sync` + 修 server shim + `static-checks.sh`
4. 可选：记录到本地 `UI_PARITY_PLAN.md` 实施笔记（不提交）

---

## 十、验证命令矩阵

| 阶段 | 命令 |
| --- | --- |
| 每次 PR | `npm --prefix web-src run typecheck` |
| 每次 PR | `npm --prefix web-src run build` |
| 每次 PR | `scripts/static-checks.sh` |
| Server 产品边界 | `node scripts/audit/audit-server-product-boundary.mjs` |
| Provider 矩阵 | `node scripts/audit/audit-ui-provider-matrix.mjs --check` |
| 契约 | `node scripts/audit/audit-web-runtime-contract.mjs --check` |
| 体积 | `node scripts/audit/audit-web-dist-size.mjs` |
| 本地冒烟 | `scripts/smoke/smoke-local.sh`（需允许启动 server） |
| 人工 UI | `docs/manual-ui-checklist.md` |

---

## 十一、工期估算

| 阶段 | 内容 | 估算（1 人全职） |
| --- | --- | --- |
| U12.0 | 地基 | ✅ 已完成 |
| U12.1 | 设计系统 | 2–3 天 |
| U12.2 | Provider 栈 | 5–7 天 |
| U12.3 | App 壳 | 2–3 天 |
| U12.4 | Settings | 3–4 天 |
| U12.5–U12.6 | Share/Usage | 4–5 天（可并行） |
| U12.8–U12.9 | 共享组件/i18n | 2–3 天 |
| U12.10 | 人工验收 | 2–3 天 |
| **合计** | | **约 3–4 周** |

---

## 十二、风险与缓解

| 风险 | 缓解 |
| --- | --- |
| 移植后编译失败（Tauri import） | `server-shims` + 禁止 `@tauri-apps/*` 进入 web-src |
| desktop `lib/api` 与 server API 形状不一致 | 薄 adapter 层；优先让组件走 `invokeCommand` |
| 体积超 4.5MB | 优先懒加载 CodeMirror/recharts（已独立 chunk）；locale 可按语言动态 import；品牌位图维持豁免登记；desktop sync 前跑 `audit-web-dist-size.mjs` |
| 双 CSS 过渡期样式冲突 | U12.1b 按切片删除 `styles.css`；禁止新增加自定义 class |
| desktop 持续迭代导致漂移 | `sync-desktop-ui.mjs --check` 纳入 CI |
| 人工验收瓶颈 | 按页面分阶段验收，不必等全量完成 |

---

## 十三、执行优先级（推荐顺序）

1. **U12.2 Server-native Provider 表单与 registry contract**（用户感知最大）
2. **U12.3 App 壳**（导航与顶栏）
3. **U12.4 Settings + U12.6 Usage**（Usage 进 Settings + recharts）
4. **U12.5 Share**
5. **U12.1b 删除 styles.css**（贯穿全程）
6. **U12.9 i18n + U12.10 人工验收**

---

## 十四、文档索引

| 文档 | 用途 |
| --- | --- |
| **本文档** | U12 完整实施计划（仓库内正式主计划） |
| `assets/contract/web-runtime-contract.json` | retained/excluded 功能契约 |
| `docs/manual-ui-checklist.md` | 人工验收清单 |
| `docs/code-audit-gap-plan.md` | 2026-07-06 代码审计缺口修复计划（Phase X） |
| `docs/architecture-refactor-plan.md` | 2026-07-06 架构优化重构计划（Phase R） |
| `AGENTS.md` | 仓库开发约定 |
| `scripts/sync/sync-desktop-ui.mjs` | Desktop → Server UI 同步 |
| `UI_PARITY_PLAN.md` | 本地-only：Phase U 历史台账（gitignore） |
| `DESKTOP_ALIGNMENT_TASKS.md` | 本地-only：后端/API/runtime 对齐笔记（gitignore） |
| `docs/remaining-work-index.md` | 本地-only：全局剩余工作索引（gitignore） |

---

## 十五、变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-07-05 | 初版：确认 2MB 体积预算、Settings 收纳 server 能力、定义 Phase U12 完整任务分解；记录 U12.0 已完成项 |
| 2026-07-07 | 体积预算上调至 4.5MB（`4503592`）：Phase U12 完整 desktop UI（四语言 locale + CodeMirror + recharts）实测 ~4.30MB；余量约 200KB，后续 sync/图标增补需门禁前置 |
| 2026-07-07 | 登记 24 个未同步品牌图标/MCP excluded 图标的体积豁免清单，维持 embedded 体积门禁 |
