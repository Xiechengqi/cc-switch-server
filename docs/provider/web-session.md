# Web Cookie / Web Session Provider

> 状态：权威。Grok Web、Perplexity Web 已实现为 Server-native、显式 Profile ID 才能创建的 `hidden` / `experimental` / `implemented` Provider；本地协议夹具为 `fixture_verified`，真实订阅仍为 `live_pending`，不得描述为真实账号已验收。

## 产品边界

Web Session 是高风险、易漂移的私有网页协议，不等同于 OAuth Account、Provider API Key 或可任意注入的额外请求头。实现遵循以下 fail-closed 边界：

- 一个 Web Session secret 归一个明确 Provider，固定使用 `/settingsConfig/webSession/cookie` 加密槽和 `credentialGeneration`。
- 不创建或复用 Account，不读取 `grok_oauth`、Gemini、API Key、extra headers 或另一个 Provider 的凭据。
- 每个 Profile 固定 HTTPS origin、POST path、Cookie allowlist、请求/响应上限、CSRF/刷新策略和流终态。
- 默认禁止 redirect、cookie jar、跨 origin 和下游 `Set-Cookie`/`Location`/`Refresh` 透传。
- 401/403 不重试、不刷新、不换 rail，只失效当前 scope 并要求管理员显式重新导入。
- 无真实凭据与脱敏 receipt 时只能写 `fixture_verified` / `live_pending`。

管理员 Web 登录的 `web_session_refresh` 与本页无关，不能作为 Provider 凭据 rail 使用。

## 双源协议审计

证据固定在 `assets/contract/web-session-source-baseline.json`，逐文件 SHA-256 和外部 commit 由 `audit-web-session-registry.mjs --check-sources` 校验。

| 候选 | OmniRoute 与 9router 共同证据 | 明显漂移/风险 | Server 处理 |
| --- | --- | --- | --- |
| Grok Web | 固定 `https://grok.com/rest/app-chat/conversations/new`、POST、`sso` Cookie、NDJSON 响应 | OmniRoute 还处理 `sso-rw`、Cloudflare Cookie、TLS 指纹和可选浏览器 clearance；模型/mode 与终态表达和 9router 不同 | allowlist 仅含经 review 的四个 Cookie 名，最低只要求 `sso`；只实现 `fast` / `expert` / `heavy` 静态目录；完整校验唯一 `modelResponse` 终态后再提交下游 |
| Perplexity Web | 固定 `https://www.perplexity.ai/rest/sse/perplexity_ask`、POST、`__Secure-next-auth.session-token`、SSE | OmniRoute 支持 chunked Cookie、Bearer 替代 rail、`Set-Cookie` 自动轮换、Firefox TLS 和新版 `COMPLETED`/`end_of_stream`；9router 协议较旧 | 只接受未分片或 `.0`–`.15` Cookie family；拒绝 Bearer 和自动 Cookie 轮换；只实现四个 reviewed model selector；严格要求 `COMPLETED` 后 `end_of_stream` |

外部项目的账号池、combo、cooldown/rotation、浏览器 Cookie jar、Cloudflare 自动获取和跨 connection recovery 均未采用。

## 独立凭据 rail

`src/domain/providers/web_session.rs` 的 `ParsedWebSessionCredential` 只接受 Cookie header 或分号分隔的 Cookie pairs：

1. 最大 16 KiB、最多 16 个 Cookie；拒绝 CR/LF/NUL 和其他控制字符。
2. 拒绝 `Bearer`、`Authorization:`、`Set-Cookie:`、JSON Cookie 导出和未知 Cookie 名。
3. 重复 Cookie、缺 required family、空值和超出 chunk 上限均失败关闭。
4. canonical header 仅在出站 transport 内可读；`Debug` 和公开摘要只显示 configured、Cookie 名、24 字符摘要和 credential generation。
5. `/settingsConfig/webSession/cookie` 已进入 Provider S2 加密凭据 inventory；它不是 `/settingsConfig/apiKey`，也不是 extra header。

四个 Server-native Profile 已进入 Registry，但全部保持 hidden：

- `claude.grok_web_session`、`codex.grok_web_session`
- `claude.perplexity_web_session`、`codex.perplexity_web_session`

它们只能由显式 Profile ID 创建，不进入 UI preset 列表。创建与更新必须复用这一专用加密槽，不得用 generic/custom Provider 绕过。

## 请求与状态边界

`guard_exact_request` 要求 method、scheme、host、port、path、query 和 fragment 与 Profile 完全一致。任何 endpoint 拼接、redirect 或跨 origin 均失败。

`WebSessionScope` 包含：

- Provider key 与 Provider revision；
- runtime fingerprint；
- credential generation；
- Web Session Profile id 与 fixed origin。

session/cache/task 只能按完整 scope 命中。凭据轮换或 runtime 变化只清理同一 Provider 的旧 scope；Provider 删除通过 `retain_scopes` 精确清理 session、task 和 invalidation 状态；401/403 只标记失败 scope 为必须重导入，其他 Provider 保持不可见且不可作为 fallback。

`state.rs` 持有独立的 no-redirect、no-cookie-jar HTTP client，并在请求提交前、401/403 失效时以及成功写入 session id 前重复核对完整 scope。Provider 变更、删除、持久化 reload 和测试 endpoint 漂移都会收敛旧 scope。Web Session 路径在通用 adapter/retry 之前终止，既不进入账号选择，也不进入跨 Provider fallback。

## 推理与模型能力

支持的下游路由只有纯文本：

- Claude Messages 与 `count_tokens`；
- OpenAI Chat Completions；
- OpenAI Responses。

Claude/Codex 的流式与非流式生命周期均会生成对应协议格式。为先证明私有网页流已经出现唯一合法终态，实现会在配置的总响应、单 frame、首帧、idle 和总超时内完整缓冲上游，再生成下游响应；这不是低延迟逐 token 透传。tools/function calls、images、attachments、`previous_response_id`、background 和非文本历史在网络前明确拒绝。`count_tokens` 使用本地保守估算且零上游。

模型发现只返回 reviewed 静态目录，`source=reviewed_web_session_catalog`、`stale=false`，每项都带 `fixture_verified`、`live_pending`、`entitlement=not_asserted`。目录不是动态 entitlement 证据：

| Driver | reviewed model id | 上游 selector |
| --- | --- | --- |
| Grok Web | `fast` / `expert` / `heavy` | 同名 mode |
| Perplexity Web | `pplx-auto` / `pplx-sonar` / `pplx-sonnet` / `pplx-opus` | `pplx_pro` / `turbo` / `claude50sonnet` / `claude50opus` |

上游网页协议不提供可信 token usage，因此 Server 只记录明确标记为 estimated 的文本 token 估算；Share request/token 计数仍按单次固定 Provider invocation 写入，不关联任何 Account。

## 首批候选合同

| Profile | Cookie allowlist | 刷新/CSRF | 上游终态 | 可见性与实现状态 |
| --- | --- | --- | --- | --- |
| `web_session.grok_web` | required `sso`；optional `sso-rw`、`cf_clearance`、`__cf_bm` | 未观察到独立 CSRF；只允许显式重导入 | 唯一 reviewed `modelResponse` 后显式 EOF；截断、重复或终态后数据为错误 | hidden / experimental / high risk / implemented / live pending |
| `web_session.perplexity_web` | `__Secure-next-auth.session-token` 或 `.0`–`.15` family | 未观察到独立 CSRF；拒绝 Bearer/Set-Cookie 自动更新，只允许显式重导入 | `COMPLETED` 后必须 `event: end_of_stream`；截断、重复或终态后数据为错误 | hidden / experimental / high risk / implemented / live pending |

网页展示名或模型映射即使同时出现在两个外部项目，也不能证明当前订阅 entitlement；只有独立 authenticated catalog 或真实 receipt 才能关闭 `live_pending`。

## 验证与开放门禁

本地实现验证：

```bash
cargo test web_session --lib
node --test scripts/audit/web-session-registry.test.mjs
node scripts/audit/audit-web-session-registry.mjs --check --check-sources
```

本地测试覆盖 Claude/Codex 流式与非流式响应、任意 transport 分块、固定 method/path/origin/Cookie、客户端头隔离、Set-Cookie 丢弃、严格终态失败、非终态 partial frame 后客户端取消仍为零重试/无 fallback、401/403 零重试、credential generation 轮换、runtime 漂移、Share 计费以及 tools/images/count_tokens 零上游。

升为 visible/stable 或把 `live_pending` 改为真实通过仍需要 Grok 与 Perplexity 各自的真实订阅脱敏 receipt、撤销/过期 Cookie 行为、当前 model entitlement 和上游漂移复验；缺任一项都不得升级成熟度。
