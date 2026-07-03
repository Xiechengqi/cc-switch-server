# 上游选择性吸收策略

## 上游来源

- 当前本地参考仓库：`/data/projects/cc-switch`
- 官方上游参考仓库：`/data/projects/cc-switch-offical`

本仓库不做整仓 merge。所有上游变更按路径和影响分类后选择性移植。

## Must Review

上游出现以下路径变更时必须审查：

- `src-tauri/src/proxy/forwarder.rs`
- `src-tauri/src/proxy/handlers.rs`
- `src-tauri/src/proxy/providers/**`
- `src-tauri/src/services/subscription.rs`
- `src-tauri/src/services/oauth_quota.rs`
- `src-tauri/src/services/usage_stats.rs`
- `src-tauri/src/provider.rs`
- `src-tauri/src/services/provider/**`
- `src-tauri/src/database/schema.rs`
- `src/config/claudeProviderPresets.ts`
- `src/config/codexProviderPresets.ts`
- `src/config/geminiProviderPresets.ts`
- `src/config/universalProviderPresets.ts`

## Optional Review

- Web provider 表单和 usage 页面。
- router/share/tunnel 相关协议。
- model pricing 和 model catalog。

## Ignore By Default

- Tauri window/tray/updater。
- Claude Desktop 专属 UI/profile 写入。
- MCP、skills、session manager。
- docs/release notes/assets。

## 导入记录模板

| 日期 | 上游提交/范围 | 分类 | 处理 | 说明 |
| --- | --- | --- | --- | --- |
| 2026-07-02 | `d73527f1` Codex chat completions bridge for OAuth responses | Must Review / proxy | 已移植(A6) | `src/proxy/transforms.rs` 新增 Codex/OpenAI Chat↔Responses 直接请求、响应和 SSE 桥接；Codex OAuth `/chat/completions` 上游归一保留 max/reasoning/response_format/tool/usage 字段 |
| 2026-07-02 | `273cc48c` Codex CN provider native Responses | Must Review / provider routing | 已移植(A6) | `scripts/audit-provider-coverage.mjs` preset 来源切到 official，上游 `openai_responses` CN preset 进入 `docs/provider-coverage.*`；server 继续由显式 `apiFormat` 驱动 Responses/Chat 路由 |
| 2026-07-02 | `784d35bd` `62c1d77e` `e1ddd86e` `8e680164` `d79fee5b` Cursor AgentService/SSE fixes | Must Review / Cursor adapter | 部分移植(C1) | server 已有显式 opt-in Claude/Codex/Gemini 文本/图片/声明工具 AgentService h2 driver、Cursor API Key exchange、AgentService endpoint override、stream interrupted usage 更新、MCP/built-in tool bridge 和 tool_result 同 h2 stream park-resume；真实 Cursor 回归仍随 C1 收尾 |
| 2026-07-02 | `cb306e95` Ollama renewal in share sync | Must Review / quota/share sync | 已移植(A4) | Ollama `/api/me` refresh 写入 subscription period，share descriptor/runtime snapshot 输出 `subscriptionExpiresAt`/`subscriptionRemainingMs` |
| 2026-07-02 | `3a7ae36e` `c0fbe902` Ollama quota display-only and display fix | Must Review / quota | 已移植(A4) | Ollama Cloud `supportsQuota` 改为 provider-specific，quota 不生成 fake `quotaPercent`，展示订阅等级为 `ollama <plan>` |
| 2026-07-02 | `e3968b72` quota summary subscription expiry | Must Review / quota | 已移植(A4) | Codex/Ollama 订阅到期写入 `quota.extraUsage` 并进入 share descriptor；真实账号 smoke 归入 F2 |
| 2026-07-02 | `6d695fe2` Codex banked reset credit time | Must Review / quota | 待移植(B10) | 第二批 quota/banked reset 展示项，当前不进入 P0 |
| 2026-07-02 | `ab09b1f7` share route/provider metadata improvements | Must Review / share descriptor | 已移植(A1) | 已补 share invocation guard、usage counters 和 descriptor counters；后续真实 direct/share metadata 差异随 F2 验收复核 |
| 2026-07-02 | `88afe26e` share request country metadata sync | Must Review / request log | 已移植(A6) | `src/proxy/forwarder.rs` 读取 `x-cc-switch-user-country`/`x-user-country` 与 ISO3 header，`state.rs` request-log batch sync 已覆盖 country 字段 |
| 2026-07-02 | `430ddf92` `dd6a951c` `de386b29` SubRouter/OpenCode Go presets | Must Review / presets | 已移植(A6) | official preset coverage 已重生成，`docs/provider-coverage.*` 包含 SubRouter 和 OpenCode Go；`docs/provider-fixtures/structures.json` 已按当前 cc-switch fork 重跑 |
| 2026-07-02 | `e6d40d0a` OpenCode Go referral/promo preset copy | Must Review / presets | 已移植(A6) | official preset coverage 使用 `/data/projects/cc-switch-offical` 当前 preset，OpenCode Go referral/promo 元数据已随 `docs/provider-coverage.*` 吸收；server 不复制前端 promo 文案组件 |
| 2026-07-02 | `d1f6c74b` usage_script credentials as explicit overrides | Must Review / provider service | 明确跳过 | server 当前只保留 `ProviderMeta.usage_script` 原始 JSON，不执行 desktop usage script，也不写 live provider config；若后续实现 usage script runner，再按该提交语义补“与 provider 主凭据相同则清空、token_plan 不清空”的规范化 |
| 2026-07-02 | `05da23e1` model-test Share path Codex stream false-positive fix | Must Review / model health | 已移植(A5) | `src/core/model_health.rs` 从 health-check usage 派生 summary，流式记录必须 `streamStatus=completed` 才计 `success`，避免 Share 路径 Codex stream false-positive |
| 2026-07-02 | `778d5b92` Claude Sonnet 5 pricing | Optional Review / pricing | 已移植(G6) | `src/core/pricing.rs` 新增 `claude-sonnet-5` 默认定价 fallback：$3/$15 input/output 与 $0.30/$3.75 cache read/write；不采用 2026-08-31 前的临时促销价 |
| 2026-07-02 | `cd9e025b` `76b8620f` Sonnet tier default/test alignment | Must Review / presets | 已吸收(G6) | provider coverage 默认 preset 源切到 `/data/projects/cc-switch` 并重跑，`claude-sonnet-5` 默认档随 upstream preset 进入 `docs/provider-coverage.*` 和 provider fixture |
| 2026-07-02 | `9079935d` NekoCode, `a8657d22` Code0.ai, `332a3c16` Amux presets | Must Review / presets | 已吸收(G6) | `scripts/audit-provider-coverage.mjs` 默认扫描 `/data/projects/cc-switch`；`docs/provider-coverage.*`、`docs/provider-fixtures/structures.json` 已重跑，NekoCode/Code0.ai/Amux preset 进入 server coverage |
| 2026-07-02 | `52a0fb4c` upstream merge | Mixed / second-round drift | 已登记(G6) | server 不整仓 merge；本轮只吸收 pricing/preset 漂移，merge 中 Codex catalog、provider service、usage stats、session UI 等差异继续由 A9/A10/H/E/C 后续任务单独偿还 |
