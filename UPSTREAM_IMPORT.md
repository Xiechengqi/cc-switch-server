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
| 2026-07-19 | `/data/projects/proxy/Claude/Claude.md` 最新对比；sub2api beta policy；TokenRouter parallel tool/SSE lifecycle；Anthropic profile billing metadata | Must Review / Claude contract + count_tokens + stateful stream + profile | 已选择性移植 | Claude OAuth contract 改为最终 body 清理后统一重算 CCH；thinking sampling 冲突归一，body/header beta 经 request-shape allowlist 决策并增加固定枚举指标，account extra headers 不能覆盖签名契约。新增 `/v1/messages/count_tokens` 与 `/claude/v1/messages/count_tokens`，只调度 `claude`/`claude_auth`/`claude_oauth` 原生 Anthropic provider，不做跨协议近似且不写生成 usage。通用跨协议 SSE 增加每请求 framing 和 Responses/Chat→Anthropic 并行 tool lifecycle，覆盖 packed arguments、任意 chunk 切分、CRLF 和 EOF 半帧。Claude profile 请求复用现有并发分支，将 `billing_type` 写入 `profile.billingSource` 并保留未知值，不推断 plan/expiry。Skill/MCP/Tauri/Desktop profile、wire casing、uTLS/JA3、tool cloaking 继续排除或证据门控。 |
| 2026-07-13 | sub2api `beta_policy.go`；CLIProxyAPI OAuth session cancellation/idempotent completion commits | Must Review / Claude OAuth beta + login session hardening | 已选择性移植 | Claude OAuth 不再无条件合并客户端 `anthropic-beta`：只允许协议内置 beta 与精确审计的 `prompt-caching-2024-07-31`、`token-efficient-tools-2025-02-19`，未知/实验 token 丢弃并写 debug 日志；非 OAuth Claude 路径保持原兼容行为。现有 server-native `OAuthLoginStore` 增加原子且幂等的 Cancelled 终态、认证 cancel API、Web invoke 接线、完成账号 ID 留存与重复完成返回；未知 state 继续 404，exchange 已启动后拒绝取消以避免授权码已消费但账号未落库。管理员 BetaPolicy 配置化、wire casing、TLS/JA3、tool cloaking 继续证据门控。 |
| 2026-07-13 | `/data/projects/proxy/Codex/Codex.md` v2；sub2api identity/tool_search/cache/WS fixes；CLIProxyAPI additional/custom tools and output restoration；cockpit-tools token authority/WS toggle | Must Review / Codex OAuth + proxy hardening | 已选择性移植 | Codex 出站身份统一为配对的官方 family，默认 `0.144.1`；device poll 可取消、幂等并防并发 exchange；refresh token 由 server 独占、重复导入拒绝且 replay 立即 relogin；OpenAI `id_token` 增加 JWKS/issuer/audience/exp 验签；workspace 只能从验证 claims 选择。补 Responses Lite additional/custom tools、tool_search 冲突/强制降级、custom 流回放、SSE/WS output 修复、wire 零值、cache-write、GPT-5.6 capability gate、非法 message item ID 清理、Windows WS reset 和 per-provider WS 回退。Images 路由沿用既有实现。TLS/JA3 继续证据门控；不读取/写穿 server host 的 `~/.codex/auth.json`，不迁移 Skill/MCP/Tauri/session manager。 |
| 2026-07-13 | `/data/projects/proxy/Claude/CLAUDE.md` 2026-07-13 增量；TokenRouter transport failover、sub2api cache usage、OmniRoute bootstrap、done-hub response headers | Must Review / Claude OAuth + proxy hardening | 已选择性移植 | Claude 自动路由在下游响应提交前可对 send/首个完整 SSE 事件/非流 body 读取失败做 3 次/10s 有界重试，并在同一逻辑请求内排除已失败 provider；显式 provider 与 share binding 保持 pinned，重试状态改为进程内 context，不再信任客户端 `x-cc-switch-claude-*retry*` 头。SSE 首个 `event:error` 支持跨 chunk 检测，提交后仍只终止流而不重放。Claude bootstrap 在登录交换后和 quota refresh 时 best-effort 拉取，仅保存规范化账号/组织字段。Anthropic/OpenAI/Gemini usage 统一为 fresh input、cache read、cache creation、output 四桶语义并补齐流/非流转换；安全响应头 allowlist 增加 request id、retry/rate-limit/priority/fast 系列。未引入 router schema 变化，也未迁移 Skill/MCP/Tauri/session manager/Desktop profile；wire header ordering、TLS/JA3、tool cloaking、54547 listener 与 MITM 继续证据门控。 |
| 2026-07-13 | `kiro.rs` `31aefbe` + `b304466` Kiro tool schema / tool JSON hardening | Must Review / Kiro proxy defense | 已选择性移植 | Kiro tool inputSchema 顶层强制 `type=object`，剥离 `oneOf`/`anyOf`/`allOf` 并从首个 object variant 恢复字段；非流式 toolUse 分片按 id 累积并在 `stop=true` 后解析，非法/半截 JSON 分别返回 `TOOL_JSON_INVALID` / `TOOL_JSON_INCOMPLETE` 502；`TOOL_SCHEMA_INVALID` 与 `TOOL_USE_RESULT_MISMATCH` 明确作为客户端终态校验错误，跳过 retry/failover 记账；补 `ksk_` API Key 错误/日志掩码。暂不吸收可选 toolCompatibilityMode 与 `e0e77c7` token_manager persist_lock，后者需按 server state/debounced save 所有权另行设计。 |
| 2026-07-10 | `/data/projects/proxy/Kiro/Kiro.md` P0-P2；kiro.rs/OmniRoute/9router/cockpit-tools Kiro OAuth + CodeWhisperer proxy references | Must Review / Kiro OAuth + proxy | 已选择性移植 | server 补齐 Kiro native refresh（Builder ID/IdC/Social/External IdP 分流）、Google/GitHub Social device flow、OIDC refresh 401 client re-register fallback、Kiro credentials.json/API key 导入与 ListAvailableProfiles 校验、IdC issuerUrl 持久化、External IdP Microsoft endpoint allowlist、嵌套凭据字段加密、getUsageLimits 回填、profileArn/region 兜底、API_KEY/EXTERNAL_IDP tokentype header、CLI endpoint 请求形态、EventStream CRC 校验和 inline `<thinking>` 拆分；保留 Claude-only Kiro forwarder capability planned，真实 Kiro non-stream/stream/usage 验收后再升级 native；明确不迁移 Kiro IDE/Tauri/MCP/skills/session manager 能力 |
| 2026-07-09 | `/data/projects/proxy/Cursor/Cursor.md` P0-P2 + §6/§7 增量审查；cc-switch-desktop Cursor OAuth/AgentService；OmniRoute/9router/ccs Cursor import + AgentService references | Must Review / Cursor OAuth + proxy | 已选择性移植 | server 补齐 Cursor 本机导入（IDE `state.vscdb` 优先、`CURSOR_STATE_DB_PATH` override、immutable SQLite URI、三平台 cursor-agent `auth.json` + `CURSOR_AGENT_AUTH_PATH` 兜底）、WorkOS Cookie `/api/auth/me` profile enrichment、Cursor AgentService 默认 native 启用并保留显式禁用开关、provider capability 从 planned 调整为 native；吸收 §6/§7 增量：Cursor token UA 统一官方登录 UA、CLI client version 从本机 state.vscdb 60min cache 探测并 fallback `cli-2026.01.09-231024f`、AgentService 增加 `traceparent`/`backend-traceparent`、timezone 读 `TZ`、OAuth/local import/profile 用同一 WorkOS subject hash 账号 ID、AgentService 429 写账号 cooldown 并让 failover 跳过、非 2xx 读取 8KB JSON 错误诊断、图片 URL 阻止 `.internal/.local/.lan` host；已核对 `TOOL_COMMIT_DIRECTIVE`、CLI AgentService header、图片 1MB/SSRF 防护和 accounts token AEAD 已在当前 tree 落地；明确不迁移 desktop Skill/MCP/Tauri/session-manager/Claude Desktop profile 能力 |
| 2026-07-09 | `/data/projects/proxy/Grok/Grok.md` P0-P2；CLIProxyAPI/sub2api/TokenRouter/done-hub/OmniRoute Grok OAuth + xAI proxy references | Must Review / Grok OAuth + proxy | 已选择性移植 | server 新增 `grok_oauth` provider type 覆盖 Claude/Codex/Gemini；补齐 xAI OAuth public client、96B PKCE、x.ai endpoint allowlist、Grok JWT/profile enrich、`~/.grok/auth.json` 导入、Responses/Chat/Images/Videos/Models 反代、xAI header/session 合约、body 清洗、reasoning/tool/encrypted_content 校验、WS Responses bridge、视频 sticky session、401/403/429/5xx 账号冷却和 Grok-only 媒体 provider 选择；明确不迁移 grok.com web cookie 逆向、Skill/MCP/Tauri/Desktop 客户端能力 |
| 2026-07-10 | `/data/projects/proxy/Claude/CLAUDE.md` P0-P2 + 二/三/四/五/六轮 review；CLIProxyAPI/sub2api/done-hub/OmniRoute Claude OAuth + proxy references | Must Review / Claude OAuth + proxy | 已选择性移植 | server 补齐 Claude OAuth refresh singleflight/退避、后台 warm-refresh 与连续 `invalid_grant` 自动隔离、`prompt=login`、统一 Claude CLI UA/CCH 常量且 CCH 默认 `cc_entrypoint=cli`、CCH seed env override、per-account stainless OS/arch profile、stream-sensitive stainless timeout、CLI header/session/user_id 合约、session-id first-user-text 种子、动态 `anthropic-beta`、非 CC system prompt 重写、billing block dedupe/TTL override、serde_json preserve_order wire 保序、缺省 `tools: []`/`max_tokens`/`temperature`/thinking `context_management`、上游 `x-request-id` 透传、Claude OAuth per-account concurrency guard（默认 8，可配置/关闭）与按占用比例负载选择、`anthropic-ratelimit-unified-reset` breaker open-until、SSE 首 chunk `event:error` breaker 信号与 3 次/10s retry ladder、400 signature/thinking 反应式降级重试、web_search 历史块过滤、Claude CLI version-gate admin rewrite、Prometheus `/metrics`、Claude CLI callback route、Claude credentials 显式 import/export；明确不迁移 Claude Web UI 反爬、MITM、Skill/MCP/Tauri/Desktop profile 自动写入，独立 54547 listener 后续按产品需求评估 |
| 2026-07-09 | `/data/projects/proxy/Codex/Codex.md` P0-P2；CLIProxyAPI `patchCodexCompletedOutput`；sub2api/TokenRouter instructions；desktop Codex WS bridge/header contract；done-hub 429/UA references | Must Review / Codex OAuth + proxy | 已选择性移植 | server 补齐 Codex OAuth CLI callback、per-account refresh lock、accounts token 字段 AEAD 加密和 `accounts.key` 备份、429 `resets_in_seconds`/`resets_at` 冷却并跳过限流账号、Codex CLI UA/originator/version 头、Responses GET WebSocket 桥、版本化 instructions 注入、`response.completed` output 回补、JWT claim profile enrich；明确不迁移 desktop Skill/MCP/Tauri 能力 |
| 2026-07-07 | desktop `forwarder.rs` Claude OAuth hot path + `claude_oauth_auth.rs` web-paste token exchange | Must Review / proxy + oauth | 已移植 | `src/proxy/claude_oauth.rs` 补齐 `beta=true`、`anthropic-beta`、billing `system` 注入、`cch` 签名；`domain/accounts/oauth.rs` + `clients/oauth/refresh.rs` 补齐 web-paste `code#state` 解析、platform token 优先与 UA；`docs/provider-coverage.md` parity notes 已更新 |
| 2026-07-07 | `d7d33e51` Ollama Codex reasoning effort clamp | Must Review / proxy | 已移植(X2) | Codex Responses→OpenAI Chat 上游归一时，Ollama 目标按 desktop `effort_value_mode="ollama"` 映射 `xhigh→max`、`minimal→low`，并保留显式 `none/off/disabled→none`；非 Ollama 目标继续透传 |
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
