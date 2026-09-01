# Code Plan delta (2026-08-23)

本页记录 2026-08-23 的 Provider/协议增量。它只描述本仓库已实现并由本地 contract/fixture 覆盖的能力；没有真实订阅凭据的项目一律保持 `live_pending`。

## 不变量

- 一个 Provider 固定绑定一个 Account，或持有一个 Provider-owned static credential。
- 请求不按 quota、cooldown、并发、错误或模型在账号间选择、轮换或重试。
- 首个 eligible 401 只可刷新或重新换取同一绑定凭据并在下游提交前重放一次；第二个 401 和提交后失败均为终态。
- 模型别名、endpoint discovery、catalog cache 和 thinking replay 只能改变当前 Provider 内的协议状态，不能选择另一个 Provider、Account、Share 或 entitlement rail。

## 本轮增量

| Code Plan | 本地合同增量 | 证据状态 | 真实验收缺口 |
| --- | --- | --- | --- |
| Gemini CLI / Code Assist | 递归规范化嵌套 function/tool schema，保留 object 结构并清理上游不接受的 JSON Schema 关键字 | fixture-verified | OAuth、project bootstrap、stream/tool、quota |
| Antigravity / Agy | 区分 server-side search 与普通 function tool；补 tier endpoint 和结构化 tier/capacity evidence | fixture-verified | OAuth、privacy、Claude/Gemini search、quota |
| GitHub Copilot | Bundle 扩为 Claude/Codex/Gemini 三 Surface；Gemini Native bridge 进入 Registry、Web 和审计清单 | fixture-verified；Gemini 未 live | github.com/GHES 分区 login、models、quota、三 Surface stream/tool/401 |
| Kimi Code | Claude Surface 改为原生 Messages/count_tokens；修正 signed-thinking replay 的 Share、用户、session、model 与 credential generation fencing | fixture-verified | device/import、catalog、quota、stream/tool/replay/refresh |
| Kiro / Amazon Q | 单个坏 tool JSON 只淘汰对应 tool；仅剩坏 tool 时 fail closed，避免污染其他合法 tool call | fixture-verified | 多认证、region/profile、stream/tool、quota、refresh |
| Qoder / COSY | wire identity 固定为 `1.24.2`；Global/CN 目录加入 GLM-5.3；client effort 只投影为 `low`/`high`/`max` | fixture-verified | Global/CN device/PAT、models、quota、三 Surface、401 |
| Cursor | 默认官方 ServerConfig discovery；官方 API-key exchange；DeepControl refresh；严格 endpoint trust、exact-scope cache 和 discovery/AgentService 共用一次 401 budget | fixture-verified；Experimental | OAuth/API-key 双 rail discovery、stream/tool/image、park/resume、rate limit |
| Alibaba Coding Plan | 新增 China 与 Global/Singapore Family；Claude 使用原生 Messages + `x-api-key`，Codex 使用 Chat Completions + Bearer；quota 明确 unavailable | registry/fixture-verified；Experimental | 两区域两协议的真实 inference、model entitlement 与错误语义 |
| Zhipu GLM Coding Plan | `glm-5.3` 只加入有 live upstream evidence 的 China/Global Codex rail，不外推到 Claude rail | registry-verified；Server live pending | Server 自身两区域 Chat stream/tool/reasoning receipt |

## 外部证据边界

- Cursor endpoint/refresh：OmniRoute `fa0cd5af1c9beec02fe0cf8eb964eb6757184e08`、`c130f2aa1ccc7aaddd7a7685bd6a0e08136dccf1`。
- Alibaba Coding Plan：9router `55628eea02eccb4d80738cbf5be342a6dbf53026`、OmniRoute `c9d4a45f1883d7daf150bbff631f3e83b41aa5b4`。
- GLM-5.3 OpenAI Coding rail：9router `8ed9da7165340150be968e968f7d9ea33902c7e3`。

这些仓库只提供协议和 Provider 类型证据。本仓库没有迁移其账号池、scheduler、sticky routing、quota 选号、组合路由、跨账号/Provider fallback、Web Cookie 逆向、Tauri、MCP 或 IDE 插件生态。

## 验收口径

本地 Rust/Node tests、mock server 和 checked-in fixture 只能把能力标为 `wired`、`statically-tested` 或 `fixture-verified`。只有 `docs/acceptance/real-acceptance-runbook.md` 记录了对应账号类型、区域、模型、non-stream、stream、tool、quota 和 refresh 的脱敏 receipt 后，才能把单项状态升级为 `live-verified`。
