# ADR：Web 运维 Terminal（Ops Shell）

> **状态**：Implemented（T1–T5）  
> **日期**：2026-07-21  
> **约束**：边缘节点 CPU/内存吃紧——按需创建、尽快回收、缓冲极小、零空载。  
> **已确认**：历史缓冲 **256KiB**；未决项一律采用边缘最优默认。

---

## 1. 决策摘要（落地值）

| 决策项 | 结论 |
|---|---|
| 产品定位 | 运维 shell：Web 登录后查看 server 环境并执行命令 |
| 实现 | Axum 进程内 `portable-pty` + WebSocket；不侧车 gotty |
| 空载 | 按钮默认显示；**未 attach 仍无 PTY**；可用配置/env 关闭整功能 |
| 会话 | 全节点最多 **1** 个 PTY；detach 不杀；idle **15min**；寿命 **2h** |
| 历史 | 内存环形 **256KiB**，进页 chunked replay |
| UI | light、Source Code Pro、自适应字号、xterm 懒加载 |
| 入口 | Terminal 按钮默认显示在 Settings/认证之间 |
| 写权限 | 默认可写（`CC_SWITCH_TERMINAL_PERMIT_WRITE`） |
| Shell | `/bin/bash` 或 `/bin/sh`，**不加 `-l`** |

---

## 2. 代码落点

| 区域 | 路径 |
|---|---|
| 后端模块 | `src/api/terminal/`（history/options/protocol/session/manager/handlers） |
| 路由 | `GET /web-api/terminal/ws`、`POST /web-api/terminal/session/end` |
| 状态 | `ServerState.terminal: OpsTerminalManager` |
| 开关 | `ServerConfig.enable_web_terminal` + `CC_SWITCH_ENABLE_WEB_TERMINAL` |
| 上下文 | `runtime.enableWebTerminal` |
| 前端入口 | `ServerDesktopApp.tsx`（Settings 与认证之间） |
| 前端页 | `web-src/src/components/terminal/TerminalPage.tsx`（lazy + xterm） |

---

## 3. 配置 / 环境变量

| 项 | 默认 |
|---|---|
| `enableWebTerminal` / `CC_SWITCH_ENABLE_WEB_TERMINAL` | 默认 **`true`**；`0/false/off` 可关 |
| `CC_SWITCH_TERMINAL_SHELL` | 自动探测 bash/sh |
| `CC_SWITCH_TERMINAL_CWD` | config dir |
| `CC_SWITCH_TERMINAL_HISTORY_BYTES` | `262144`（夹紧 64KiB–1MiB） |
| `CC_SWITCH_TERMINAL_IDLE_DETACH_SECS` | `900` |
| `CC_SWITCH_TERMINAL_MAX_LIFETIME_SECS` | `7200` |
| `CC_SWITCH_TERMINAL_PERMIT_WRITE` | `true` |

---

## 4. 阶段完成

| 阶段 | 状态 |
|---|---|
| T0 边缘默认 | 已确认 |
| T1 导航 + 懒加载入口 + enable 显隐 | 完成 |
| T2 PTY + WS attach/detach | 完成 |
| T3 256KiB history + replay | 完成 |
| T4 light / Source Code Pro / 自适应字号 | 完成 |
| T5 idle/lifetime/结束会话/测试 | 完成 |

---

## 5. 验收要点

1. 默认无 Terminal 按钮；开启开关后出现在 Settings 与认证之间。  
2. 进入终端可交互；返回首页不断开后台命令。  
3. 再进入可看到缓冲内历史输出。  
4. 「结束会话」kill PTY；无前台 15 分钟或最长 2 小时自动回收。  
5. 第二用户 attach → busy；主包不预载 xterm（独立 chunk）。

---

## 6. 决议

- [x] 接受边缘紧缩默认值（历史 256KiB）  
- [x] T1–T5 实施完成（2026-07-21）
