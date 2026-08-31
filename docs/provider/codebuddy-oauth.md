# CodeBuddy OAuth 单账号反代（实施规划）

> **状态：规划，未实现。** 仓库中尚不存在 `codebuddy_oauth` ProviderType、driver、profile 或账号域模型。本文中的"必须/固定/不允许"是**实现约束**，不是既有行为描述，不能据此认为已有验收结论。
>
> 本文只写**已证实**的部分。所有未决项、以及它们各自的判定标准与抓包方法，见 workbuddy-cliproxy 仓库的 `codebuddy-open-questions.md`，编号 `U1`–`U15` 在两份文档间通用。
>
> **2026-08-31 更新：U1–U15 已在国际版真实账号上全部关闭**（CLI v2.142.0 + mitmproxy 抓包）。本文相关章节已回填实测结论，标注 **【D】**。仍未覆盖的三项（国内站数据面、企业账号、图像模型）见 §9。

本文定义 cc-switch-server 的 `codebuddy_oauth` Provider 边界。一个 CodeBuddy Provider 固定绑定一个 CodeBuddy Account；Server 不实现账号池、轮询、权重、按配额/并发选号、自动切站，或跨 Provider fallback。

---

## 0. 证据来源与置信度标注

| 代号 | 来源 | 采集方式 | 时间 |
|---|---|---|---|
| **A** | CodeBuddy CLI npm 包：`dist/codebuddy.js`（约 25 MB bundle）、`product.json`、`product.internal.json` 等 overlay | `npm pack` 解包后静态阅读 | 2026-08-31 |
| **B** | 国际版官方文档 `https://www.codebuddy.ai/docs/zh/cli/*` | `curl`（`WebFetch` 对该域名被拦截） | 2026-08-31 |
| **C** | `/data/projects/proxy/workbuddy-cliproxy` 生产实现（国内站已跑通） | 直读源码 | — |
| **D** | **国际版真实账号抓包**：CLI v2.142.0 + mitmproxy 11.0.2 正向代理，个人账号（`enterpriseId: ""`） | 实测流量 + bundle 定点反查 | 2026-08-31 |

正文中的标注含义：

- **【D】** — **真实流量实测**，置信度最高。脱敏证据见 `workbuddy-cliproxy/codebuddy-open-questions.md` §4 与 `~/cbcap/redacted/`。
- **【A】/【C】** — 代码直读或双证，可直接实现。
- **【B】** — 仅官方文档声明，未经代码或流量交叉验证；实现时按文档写，但列入回归观察。
- **【Un】** — 未证实，见 open-questions 文档对应条目。**U1–U15 已于 2026-08-31 全部关闭**；正文中残留的 `Un` 引用均已改写为对应的【D】结论。

---

## 1. Provider 形态与站点模型

### 1.1 单一 ProviderType

新增**一个** `ProviderType::CodeBuddyOAuth`（wire 值 `codebuddy_oauth`），站点是**账号身份的一部分**，不是 ProviderType 的分裂维度。

这与仓库既有惯例一致：`src/domain/providers/model.rs` 的 23 个变体全部按 **credential scheme** 分裂（`claude_auth`/`claude_oauth`、`cursor_oauth`/`cursor_apikey`、`deepseek_account`/`deepseek_api`），**没有任何一个按 region 分裂**。国内/国际两站共用同一套 OAuth 凭据机制，拆成两个 ProviderType 会在契约、注册表、UI、覆盖率矩阵上产生一整套无意义的重复。

直接同构参照是 `qoder_cosy`：单一 ProviderType，站点由 `QoderSite { Global, Cn }` 承载于账号 Profile（`src/domain/qoder.rs:327`）。

### 1.2 站点枚举

```rust
pub enum CodeBuddySite { Intl, Cn }
```

本期只实现这两个。CLI 侧实际存在五个环境（`ProductEnviroment`，**原文即拼写为 Enviroment**）：`Internal | External | IOA | Cloudhosted | Selfhosted`【A】。`IOA`（腾讯内网 SSO）、`Cloudhosted`（专享）、`Selfhosted`（私有化）依赖企业侧域名与准入，不在本期范围，但枚举与 `parse()` 必须为它们预留显式拒绝分支，而不是 fallthrough 到 `Intl`。

### 1.3 站点 Profile 表

| 字段 | `Intl` | `Cn` | 来源 |
|---|---|---|---|
| `endpoint` | `https://www.codebuddy.ai` | `https://copilot.tencent.com` | 【A】`product.json` / `product.internal.json` |
| `staging_endpoint`（不启用，仅记录） | `https://staging-codebuddy.tencent.com` | `https://staging-copilot.tencent.com` | 【A】 |
| ~~`origin_referer`~~ | **不适用** | `https://www.codebuddy.cn` | 【D】国际版 CLI 不发 `Referer`/`Origin`，此字段不应存在于 Intl profile |
| `internet_environment` | 不设置（默认） | `internal` | 【B】`CODEBUDDY_INTERNET_ENVIRONMENT` |
| `login_methods` | Google / GitHub | 微信 | 【A】登录方式表 |
| `api_key_portal`（仅用于 UI 提示） | `https://www.codebuddy.ai/profile/keys` | `https://copilot.tencent.com/profile/` | 【B】 |

`product.json` 中站点判定所用的域名清单【A】：

```
internalDomain     : copilot.tencent.com, staging-copilot.tencent.com,
                     www.codebuddy.cn, staging.codebuddy.cn,
                     www.workbuddy.cn, staging.workbuddy.cn
externalDomain     : www.codebuddy.ai, staging-codebuddy.tencent.com
iOADomain          : tencent.sso.copilot.tencent.com, ...
cloudHostedDomain  : *.sso.copilot.tencent.com, *.copilot.qq.com, ...
```

文档另行确认 `workbuddy.ai` 也属国际 endpoint 集合【B】（见 §6.4）。`www.workbuddy.cn` 归入 `internalDomain`【A】。因此 workbuddy 与 codebuddy 是同一平台的两个品牌门面，**不构成第三个站点**，共用同一套 endpoint 与协议。

**站点是不可变的账号属性。** 请求中的模型、403、配额或网络错误都不能改变站点；不存在"国际失败回落国内"的路径。

### 1.4 关于"只在认证模块加站点切换"

这个想法在架构上不成立，实现时不要按它设计。站点必然外溢到认证模块之外的三处，且 `qoder_cosy` 已经全部踩过：

1. **运行时缓存作用域** —— 参照 `src/proxy/qoder_runtime.rs:52` 的 `QoderRuntimeScope::derive(...)`，site 是摘要输入之一。不含 site 的 scope 会让国内/国际的 session、目录、conversation 互相命中。
2. **模型 capability 投影** —— 参照 `qoder_model_aliases(site)`、`thinking_capability(site, key)`、`qoder_reasoning_efforts(site, key)`、`qoder_context_capability(site, key)`。CodeBuddy 两站模型集差异极大（§6.3）。
3. **出站身份** —— `Referer` 与 endpoint 按站点分叉（§1.3、§5.2）。

真正"只在认证模块"的部分只有一处：管理员在建号时选站点，即 `src/api/accounts.rs:2020` 那种 `QoderSite::parse(input.site)` → `start_device_flow(site, now)` 的入口。**契约层（`assets/contract/provider-registry.json`）可以完全 site 无感知** —— qoder 的契约里 site 一次都没出现，CodeBuddy 照做。

---

## 2. 账号身份与 Profile

### 2.1 Profile 结构

对齐 `QoderAccountProfile`（`src/domain/qoder.rs`）：

```rust
pub struct CodeBuddyAccountProfile {
    pub site: CodeBuddySite,
    pub uid: String,             // login/account 返回的用户 ID
    pub enterprise_id: String,   // 个人账号为空串
    pub name: String,
    pub email: String,
    pub nickname: String,        // 【D】accounts 返回 `nickname`
    pub account_type: String,    // 【D】`type` = "personal" | ...
    pub client_version: String,  // 固定 CLI 版本，见 §5.2
    pub product_platform: String,// 【D】auth/state 的 platform = "CLI"
}
```

**U6、U8 已关闭【D】。** `platform` 解析为 `authentication.attributes.platform ?? configuration.platform` = **`"CLI"`**，与国内版一致；`isWorkbuddyAuthenticationPlatform` 只在 workbuddy 品牌 overlay 下才成立。

`GET /v2/plugin/accounts` 实测字段：`uid`（UUID，等于 JWT `sub`）、`nickname`、`uin`、`type`（`"personal"`）、`lastLogin`、`isCreator`、`isAdmin`、`pluginEnabled`、`deployStatus{}`、`accountType`、`sso{domain, domainModifiedTimes}`、`idp`、`areaInfoComplete`、`oneidAccountId`、`isCurrentOneIdEnterprise`、`isCurrentOneIdPersonal`、`isFirstLogin`。

> **注意：响应中没有 `enterpriseId` 字段。** 个人账号的 `enterprise_id` 只能从 `/v3/config` 响应里取（实测为空串 `""`），不能指望 accounts 接口提供。identity 派生（§2.2）依赖它，因此**登录落库前必须先调一次 `/v3/config`**，否则 `enterprise_id` 无来源。这是 §3.5 "三者缺一即失败"之外的第四个必要条件。

### 2.2 identity 派生（必须含 site）

```
account_id = "codebuddy-" + hex12( sha256( site_str ‖ 0x00 ‖ uid ‖ 0x00 ‖ enterprise_id ) )
```

**site 必须进入摘要。** workbuddy-cliproxy 现有实现是 `sha256(uid ‖ 0x00 ‖ enterpriseId)[:12]` → `workbuddy-<hex12>`【C】，不含 site。CodeBuddy 国内与国际是**两套独立的用户体系**，uid 空间不保证不相交；一旦相交，两个不同账号会派生出同一个 account id，凭据互相覆盖。这是移植时必须修掉的缺陷，不能原样搬。

`enterprise_id` 为空串时照常参与摘要（不做 skip），保证个人账号与"企业 id 恰好为空"的账号派生一致且稳定。

账号 identity generation 变化后，旧 Provider 绑定返回冲突，必须由管理员显式重新绑定 —— 与 `kimi_code`、`qoder_cosy` 一致。

---

## 3. OAuth：`cli-external-link` 浏览器链接流

### 3.1 流程类型

两站**都是** `cli-external-link`【A】。`product.json` 的 `authentication.type = "cli-external-link"`，而 `product.internal.json` 这个国内 overlay **完全没有 `authentication` 键** —— 国内继承国际的认证块，两站认证机制逐字节相同。

CLI 只负责"打开一个链接"，**微信 / Google / GitHub 的差异发生在浏览器落地页，不在 CLI 协议里**。因此反代侧两站共用同一段代码，无分支。

两个需要主动排除的误导项【A】：

- `WxAuthenticationProvider` —— 仅当 `authentication.type === "wechat"` 时启用，且其 refresh 路径是 `/v2/auth/token/refresh`（**不带 `/plugin` 前缀**）。这是另一条产品线，与本 Provider 无关。
- `performQrCodeAuth` / `fetchQRCode` —— 打的是 `ilink/bot/get_bot_qrcode?bot_type=`，是**微信机器人绑定**（WeChatReply / WeComReply 工具），不是账号登录。

### 3.2 路径模板

CLI 内所有认证路径都由 `prefixPath` 模板拼出，`prefixPath` 在 `product.json` 中固定为 `/plugin`【A】：

```js
`/v2${this.prefixPath}/auth/state?platform=${platform}`   // POST
`/v2${this.prefixPath}/auth/token?state=${state}`         // GET，轮询
`/v2${this.prefixPath}/login/account?state=${state}`      // GET，轮询
`/v2${this.prefixPath}/auth/token/refresh`                // POST
`/v2${this.prefixPath}/login/enterprise`                  // POST
`/v2${this.prefixPath}/login/enterprise/${enterpriseId}`  // POST
`/v2${this.prefixPath}/accounts`                          // GET
```

展开后（两站仅 host 不同）：

| 用途 | 方法 | 路径 |
|---|---|---|
| 申请登录 state | `POST` | `{endpoint}/v2/plugin/auth/state?platform={platform}` |
| 轮询 token | `GET` | `{endpoint}/v2/plugin/auth/token?state={state}` |
| 轮询账号信息 | `GET` | `{endpoint}/v2/plugin/login/account?state={state}` |
| 刷新 token | `POST` | `{endpoint}/v2/plugin/auth/token/refresh` |
| 列出可切换账号 | `GET` | `{endpoint}/v2/plugin/accounts` |
| 切换企业账号 | `POST` | `{endpoint}/v2/plugin/login/enterprise[/{id}]` |

`platform` 实测为 **`CLI`**，两站一致【D】（解析式 `authentication.attributes.platform ?? configuration.platform`）。bundle 中另有 `isWorkbuddyAuthenticationPlatform(p) → p?.trim().toLowerCase().startsWith("workbuddy")` 分支【A】，仅在 workbuddy 品牌 overlay 下成立，本 Provider 不触发。**U6 已关闭。**

### 3.3 未认证标记头

`auth/state` 与两个轮询请求必须带反向标记头【A】，告诉服务端"本请求刻意不带身份"：

```
X-No-Authorization  : true
X-No-User-Id        : true
X-No-Enterprise-Id  : true
X-No-Department-Info: true
```

漏掉这些头会让网关按已认证请求处理并拒绝。

### 3.4 Cookie 亲和性

`auth/state` 与其后的所有轮询**必须复用同一个 cookie jar**【C】。服务端把浏览器侧登录与 state 绑定在会话上，换 jar 会导致轮询永远拿不到结果。实现上每个 login flow 持有独立的 HTTP client（参照 workbuddy-cliproxy `newLoginClient`），flow 结束或过期即销毁。

### 3.5 轮询语义与 TTL

- `auth/token?state=` 在授权未完成时返回业务码 **`11217`**（`RetryFetchToken`），完成时返回 `code: 0` 与凭据。**U2 已关闭【D】**——bundle 中的 `loopGetToken` 明确 `catch code === 11217 -> continue`，其他错误一律 throw。
- 账号信息另有一条轮询：`GET /v2${prefixPath}/login/account?state=` 携带 `Authorization: Bearer {accessToken}`，未就绪时返回 **`12151`**（`RetryFetchAccount`）继续轮询；HTTP 401/403 有限次重试后 throw【D】。注意它与稳态期的 `GET /v2/plugin/accounts` 是**两个不同端点**。
- **不可把 11217 / 12151 当作失败** —— 它们是轮询继续信号。终态授权错误另有一组码，见 §8。
- login flow TTL **10 分钟**【C】。
- 每个 state 只允许一个 poll lease；并发 poll 返回 in-progress，不并行消费授权结果（与 `kimi_code` device flow 同口径）。
- Server 必须对轮询间隔、最大轮询时长、单次请求超时（30s）与响应体上限（256 KiB）做有界归一。
- 登录完成必须同时拿到 access token、refresh token，以及 `login/account` 中稳定的 `uid`。三者缺一即失败，不落库。

### 3.6 控制面接口

与 `kimi_code`（`/api/accounts/kimi/device/*`）、`qoder_cosy`（`/api/accounts/qoder/device/*`，`src/api/mod.rs:503-514`）同构：

```
POST /api/accounts/codebuddy/login/start    { site: "intl" | "cn" }  -> { flowId, authUrl, expiresAt }
POST /api/accounts/codebuddy/login/poll     { flowId }               -> pending | done | failed
POST /api/accounts/codebuddy/login/cancel   { flowId }
```

`site` 只在 `start` 出现，之后由 flow 与账号 Profile 携带 —— 这就是 §1.4 所说"认证模块里的站点切换"的全部范围。`cancel` 删除当前 flow；过期 flow 在访问时清理；所有 flow 写操作由 `state.rs` 域方法封装。

---

## 4. Refresh

```
POST {endpoint}/v2/plugin/auth/token/refresh
X-Refresh-Token       : {refresh_token}
X-Auth-Refresh-Source : plugin
```

【A】。注意 workbuddy-cliproxy **没有**发送 `X-Auth-Refresh-Source: plugin`【C】；`workbuddy-switch` 同样不发且能正常刷新【C】—— 两个独立国内实现佐证该头**服务端可选**，但官方 CLI 一直在发，新实现按官方发送。

- 提前量：access token 到期前 **5 分钟**触发【C】。
- 刷新结果必须能解析出与原账号一致的 `uid`。不一致时返回身份冲突并要求重新登录，**不能把新 token 写入旧账号**。
- refresh token 轮换只更新当前账号；上游未返回替换值时沿用原值。
- **U7 已关闭【D】**。实测请求同时携带 `X-Refresh-Token`、`X-Auth-Refresh-Source: plugin`、`Authorization`、`X-User-Id`、`X-Domain`、`X-Product`，body 为 `{}`。响应：

  ```json
  {"accessToken":"...","expiresIn":31530425,"refreshExpiresIn":31530425,
   "refreshToken":"...","tokenType":"Bearer","sessionState":"...","scope":"...","domain":"..."}
  ```

  **refresh token 会轮换**（返回值与送入值不同），实现必须写回新值，否则下次刷新失败。`expiresIn` 约 365 天。
- **`12153 invalid_grant`（`Session doesn't have required client`）**【C】：refresh token 闲置数天后服务端会清理会话，刷新失败且**无法恢复**，只能重新登录。`workbuddy-switch` 因该真实事故改为每日无条件刷新一次保活。
  → 仅靠"到期前 5 分钟刷新"不足以规避（access token `expiresIn` 约 365 天，正常路径下一年都不会触发刷新）。实现应提供**周期性保活刷新**，并把该失败与普通过期区分（账号标 `needs_relogin` + 原因），而不是静默重试。国际站的清理阈值未实测。
- 凭据由加密的 `accounts.json` 保存，Provider 不保存第二份 token。

---

## 5. 数据面

### 5.1 端点

```
POST {endpoint}/v2/chat/completions
```

**国内、国际逐字节相同**——国际站实测为 `POST https://www.codebuddy.ai/v2/chat/completions`。**U1 已关闭【D】**，国际版可用性不再被端点问题阻塞。

### 5.2 出站身份头

实测（国际版 CLI v2.142.0 抓包）：

| 头 | 值 | 上游是否校验 |
|---|---|---|
| `Authorization` | `Bearer {access_token}` | **是（唯一强制项）** |
| `X-User-Id` | `{uid}`（UUID，等于 JWT `sub`） | 否 |
| `X-Domain` | `www.codebuddy.ai` | 否 |
| `X-Product` | `SaaS` | 否 |
| `X-Request-ID` | 每请求唯一 | 否 |
| `X-Requested-With` | `XMLHttpRequest` | 否 |
| `User-Agent` | `CLI/{ver} CodeBuddy/{ver}` | 否 |
| `X-Enterprise-Id` | 企业账号时携带 | 未测（个人账号） |

会话类头（CLI 另发，非必需）：`X-Conversation-ID`、`X-Conversation-Request-ID`、`X-Conversation-Message-ID`、`X-Root-Request-ID`、`X-Agent-Intent: craft`、`X-Agent-Purpose: conversation`、`X-Agent-Type: main`、`X-IDE-Type/Name/Version`、`X-Private-Data: false`、`x-codebuddy-request: 1`，外加 stainless SDK 头（openai npm 6.25.0）与 B3 / traceparent 链路头。

> **重要修正【D】**：国际版 CLI **完全不发送 `Referer` 或 `Origin`**。此前基于国内实现推测的 `origin_referer` 站点字段在国际侧没有依据，实现**不应**注入这两个头。**U9、U12 已关闭。**

最小必需头集合 = `Authorization` + `Content-Type: application/json`。其余全部可省。实现仍建议带上 `X-User-Id` / `X-Domain` / `X-Product` 以贴近官方形态，但不得依赖它们生效。

**账号 `extraHeaders` 不允许覆盖 `Authorization` 或任何 `X-User-Id` / `X-Enterprise-Id` / `X-No-*` 身份头。** 最终 CodeBuddy 身份在通用账号 header 合并后重新覆盖 —— 与 `kimi_code` 对 `X-Msh-*` 的处理同口径，避免历史配置改变认证身份。

### 5.3 强制流式

**两站均拒绝非流式请求【D】**。`stream:false` 的响应是 **HTTP 400** + body `{"code":11101,"msg":"Non-stream chat request is currently not supported"}` —— 注意是 400，**不是** `200 + code`。实现始终在请求体注入 `"stream": true`，再按下游 Surface 需要决定是否聚合回非流式响应。**U2、U3 已关闭。**

### 5.4 首条消息必须是 system prompt

**新增硬约束【D】**：若 `messages[0].role != "system"`，上游返回 **HTTP 400** + `{"code":11128,"msg":"first message is not system prompt"}`。

这是反代必须处理的强制项：下游若送来一个不以 system 开头的对话（Anthropic Messages 形态常见——system 是独立顶层字段），实现**必须**合成或前置一条 system 消息，否则请求必然失败。

### 5.5 请求体 gzip

`ProductFeature.RequestBodyGzip` 在国际版 `product.json` 中**静态即为 `true`**【D】，不依赖云端下发（云端只下发 `CodeAdoptionRate` / `TodoAssistantDelegate` 两个 flag）。实测 CLI 的 chat 请求确实带 `Content-Encoding: gzip`。

但 `GzipRequestProcessor` 有三重门控，其中第二条对本项目决定性：

1. `productFeatures.RequestBodyGzip` 为 true；
2. **`CODEBUDDY_BASE_URL`（env 或 settings）一旦设置即跳过压缩**；
3. 目标模型在目录中自带 `url` 时跳过。

> **对实现的结论**：cc-switch-server 作为 `CODEBUDDY_BASE_URL` 指向的反代时，**永远不会收到 gzip 请求体**，入站无需实现解压。出站直连上游时，gzip 可选（上游同时接受未压缩体）。**U13 已关闭。**

### 5.6 工具调用与推理的线格式

**请求**为标准 OpenAI `tools[].function` 形态；CLI 另带若干非标准字段：

```json
{"model":"default-model","temperature":1,"max_tokens":24000,"stream":true,
 "stream_options":{"include_usage":true},"reasoning_effort":"high","tools":[...],
 "messages":[{"role":"system","content":"..."},
             {"role":"user","agent":"cli","content":[{"type":"text","text":"..."}]}]}
```

- `reasoning_effort` 是**顶层字符串**（非 `reasoning:{effort}` 对象）。
- `stream_options.include_usage=true` 是末帧 usage 的来源；实现若需用量统计必须带上。
- 回填轮的 assistant / tool 消息带非标准 `messageId`、`model`、`requestModelId`、`requestModelName`、`traceId`、`conversationRequestId`、`agent`。这些**不是必需的**，反代可不透传。

**响应 SSE**：`object: "chat.completion.chunk"`，delta 为**稠密**结构，每帧都带全部键 `content` / `reasoning_content` / `function_call` / `refusal` / `tool_calls` / `extra_fields`。

> **关键不对称【D】**：推理内容在响应里叫 **`delta.reasoning_content`**，但下一轮请求的 assistant 消息里叫 **`reasoning`**。做多轮工具循环的实现必须完成这层改名，否则思维链在回填时丢失。

`tool_calls` 为标准增量（首帧 `id`+`name`，后续帧只递增 `arguments`）；**首帧的 `index` 是否存在随模型而变**，按 `index` 归并的实现须容忍首帧缺失。终帧 `finish_reason` ∈ `{tool_calls, stop, length}`，随后独立一帧携带 `usage`，最后 `data: [DONE]`。**U10 已关闭。**

### 5.7 usage 字段

末帧 `usage` 共 13 个字段【D】：

```
prompt_tokens, completion_tokens, total_tokens, credit,
completion_thinking_tokens, completion_tokens_details{...}, prompt_tokens_details{...},
cached_tokens, cache_read_input_tokens, cache_creation_input_tokens,
prompt_cache_hit_tokens, prompt_cache_miss_tokens, prompt_cache_write_tokens
```

`credit` 是计费信号，`completion_thinking_tokens` 单独计推理 token。**U15 已关闭。**

---

## 6. 模型目录

### 6.1 权威通道：`GET {endpoint}/v3/config`

**这是本轮最重要的发现，并推翻了早前"CodeBuddy 无模型发现端点、只能静态白名单"的判断。**

`CloudProductProvider`【A】：

```
GET {endpoint}/v3/config          # 注意：根路径，不带 /v2/plugin 前缀
timeout : 5s
params  : { repos: [...本地 git 仓库 URL...] }
关闭开关 : CODEBUDDY_REMOTE_CONFIG_DISABLED=1|true
```

响应体形如 `{ data: { models: [...], agents: [...], productFeatures: {...}, tokenUsageThresholds: {...} } }`，与本地 `product.json` 合并后生效。合并语义是关键：

```js
mergeModelsById(local, remote) =>
    remote.map(m => local.has(m.id) ? { ...local.get(m.id), ...m } : m)
```

**返回值以 remote 列表为准**，本地条目只用于补齐缺失字段。也就是说 `/v3/config` 一旦下发 `models`，它就是有效目录 —— 语义上非常接近 Qoder `/algo/api/v2/model/list` 的 bound-account-authoritative 模型。

准入门槛（`CloudPreProductProvider`）【A】：会话必须有 `accessToken`；若 `tokenType === "ApiKey"` 则 token 需以 `ck_` / `pt_` 开头，否则整条通道被置 `disabled`。**OAuth 取得的 accessToken 直接放行**，正是本 Provider 的场景。

官方 CLI 侧的容错链【A】：per-user 内存缓存 8 分钟 → 磁盘缓存 `cloud_product_config_cache` → 退避重试 `[1,2,4,8,16]s`；空 payload 被 `LastGoodGate` 拒绝并回落上一次好配置。

请求头含 `x-domain`、`x-enterprise-id`、product、request-id、user-id【A】。

> **实现约束：不转发 `params.repos`。** 官方 CLI 会把本地 git 仓库 URL 作为查询参数上报，反代没有任何理由复刻这一行为。

### 6.2 目录策略（对齐 kimi_code / qoder_cosy 口径）

**U4/U5 已关闭【D】**：`/v3/config` 对个人 OAuth 账号（`enterpriseId: ""`）实测返回 **35 个模型**，非空且字段完整。权威目录路线成立，本节即为最终状态。

模型条目 schema【D】：

```
credits, descriptionEn, descriptionZh, disabledMultimodal, id,
maxAllowedSize, maxInputTokens, maxOutputTokens, name, onlyReasoning,
reasoning{effort|defaultEffort, summary, canDisableThinking, supportedEfforts[]},
relatedModels{lite, reasoning}, supportsImages, supportsReasoning,
supportsToolCall, tags[], temperature, top_p, vendor
```

能力字段比 Qoder 的目录更完整（含 reasoning effort 档位、多模态开关、上下文与输出上限、温度默认值），足以直接投影出 reviewed profile 的全部能力位，无需推测。同一响应还含 `agents`(16)、`enterpriseId`、`productFeatures`。

- 目录按 App、Provider revision/runtime fingerprint、Account、identity generation、token generation、**site** 隔离，并 single-flight 获取。
- **成功空目录是权威结果**，保持为空，不与 Provider 配置、静态 alias 或其他账号目录合并。
- 只有 network / 408 / 429 / 5xx 才允许读取**完全相同作用域内**、24 小时硬上限内的 stale cache。
- 认证错误、超限响应、坏 JSON、未知成功结构、身份 scope 漂移，以及"上游非空但全部模型均不在 reviewed allowlist"，一律 fail closed 并清理当前 scope。
- **运行时不存在静态 entitlement fallback** —— 与 `kimi_code` 条款一致。`product.json` 内嵌目录只作为**字段补全的元数据源**（`mergeModelsById` 的 local 侧），不作为 entitlement 来源。这个区分是本 Provider 能通过该条款的原因。
- `/v1/models` 只在当前 Share 选中的 CodeBuddy Provider 范围内，公开上游目录与 reviewed allowlist 的交集，不参与 Provider 或账号选择。

### 6.3 站点模型集差异

`product.json` 声明的模型数：**国际 35 / 国内 23**【A】。二者几乎不重叠：

- 国际：`gpt-5.6-sol/terra/luna`、`gpt-5.5`、`gpt-5.4`、`gpt-5.3-codex`、`gpt-5.1-codex(-mini)`、`gemini-3.1-pro`、`gemini-3.5-flash`、`gemini-3.0-flash`、`gemini-3.1-flash-lite`、`gemini-2.5-pro/flash`、`glm-5.3/5.2/5.0`、`kimi-k3/k2.6/k2.5`、`minimax-m3`、`deepseek-v3-2-volc`、`hy3` + 图像/视频模型
- 国内：`deepseek-v4-pro/flash`、`deepseek-v3-2-volc`、`minimax-m3/m2.7/m2.5`、`glm-5.2/5.1/5.0/5.0-turbo/5v-turbo/4.7/4.6/4.6v`、`kimi-k3-1/k2.7/k2.6/k2.5/k2-thinking`、`hy3`、`hunyuan-chat`、`hunyuan-image-v3.0-art`

**默认别名两站不同名**【A】：国际是 `default-model`、`default-model-lite`、`fast-model`、`balanced-model`、`primary-model`、`deep-model`；国内是 `default`。

这一条直接影响契约：`qoder_cosy` 的 `defaultUpstreamModel` 可以取站点中立的 `"auto"`，**CodeBuddy 不行**。契约里的 `defaultUpstreamModel` 必须要么留空由运行时按 site 解析，要么取一个两站都存在的具体 id（当前不存在这样的 id）。推荐做法：契约写 `defaultUpstreamModel: ""`，由 `qoder_runtime.rs` 同构的 `codebuddy_runtime` 按 site 解析默认别名。

alias 表按 site 集中维护，但只有对应 site 下 live-enabled 的 route 才发布公开 alias。上游出现未知 route 时保留 exact id，便于观察新 entitlement，同时不猜测 reasoning、context、图像等能力。

#### 6.3.1 目录 ≠ 权限（实测，U11）

**35 个目录条目并非都可调用【D】。** 对国际个人账号逐个打点的结果：

- **放行（14）**：`default-model`、`fast-model`、`balanced-model`、`deep-model`、`gemini-3.1-pro`、`gemini-3.5-flash`、`gemini-3.1-flash-image`、`gemini-2.5-flash-image`、`glm-5.3`、`glm-5.2`、`glm-5.0`、`hy3`、`kimi-k3`、`kimi-k2.6`、`kimi-k2.5`
- **`11102` `model [x] service info not found`**：`default-model-lite`（服务端改写为 `codewise-default-cw-api-3`）、`gemini-2.5-flash` / `gemini-2.5-pro`（改写为 `-us-east1` 后仍未开通）、`gpt-5.1-codex(-mini)`、`gemini-3.0-flash`、`gemini-3.1-flash-lite`、`deepseek-v3-2-volc`、`hunyuan-*`
- **`11133` `the request parameters were rejected by the model provider`**：`gpt-5.5`、`gpt-5.4`、`gpt-5.3-codex`、`gpt-5.6-sol/terra/luna`、`primary-model`。换 `reasoning_effort` / `reasoning{}` / `max_completion_tokens` 均无效，判定为**账号无权限**而非参数问题。
- `gemini-3.0-pro-image` → `Backend [aiart] is not supported`

另有 CLI 自身的白名单：`codebuddy --model` 只接受 **17 个** id（`default-model, fast-model, balanced-model, primary-model, deep-model, gpt-5.6-sol/terra/luna, gpt-5.5, gpt-5.4, gpt-5.3-codex, gemini-3.5-flash, glm-5.3, glm-5.2, kimi-k3, kimi-k2.6, minimax-m3`）。

> **对实现的结论**：`/v3/config` 是**目录**而非**权限**。reviewed alias 表应以「目录 ∩ CLI 白名单 ∩ 实测放行」为准，并把 `11102` / `11133` 映射为"模型不可用"而非通用 400 —— 否则用户会在一个目录里可见、实际调不通的模型上反复失败。同时这也意味着**目录内容随账号而变**，不能硬编码。

### 6.4 企业目录（可选，本期不实现）

```
GET {endpoint}/console/enterprises/{enterpriseId}/config/models
```

`ModelsProductProvider`【A】。`enterpriseId` 缺失时直接跳过 —— **个人账号永远不触发**。5 分钟缓存，失败退避 30s–300s。仅当后续要支持企业账号时实现。

### 6.5 站点条件能力（可能影响目录投影）

文档确认存在服务端/端侧按站点分叉的能力开关【B】：

> `CODEBUDDY_ARTIFACT_ENABLED` —— 判定顺序：国际 endpoint（`codebuddy.ai` / `workbuddy.ai` / `staging-codebuddy.tencent.com`）下该能力**恒关闭、本变量无法覆盖**。

bundle 中的 `ProductFeature` 全集【A】：`Artifact`、`ImageGen`、`ImageEdit`、`VideoGen`、`RemoteControl`、`RequestBodyGzip`、`SkipToolCallSupportCheck`、`ModelRateLimitCap`、`DisableMultimodalGeneration`、`EnableEnterpriseCustomModelPolicy`、`CustomModelIdPrefix`、`SwitchBySession`、`ShareLink`、`SkillManage`、`WebFetchRemoteApi` 等 27 项。

对本 Provider 有直接影响的是 `SkipToolCallSupportCheck`、`ModelRateLimitCap`、`RequestBodyGzip`、`DisableMultimodalGeneration`。

**U13 已关闭【D】**：实测国际版 `product.json` 静态声明 **75 个** flag（`RequestBodyGzip: true`、`SkipToolCallSupportCheck: true`、`CustomModelIdPrefix: true`、`InternationalLogin: true`；`DisableTlsVerification` 与 `ModelRateLimitCap` **不存在**），而云端 `/v3/config` 对个人账号**只下发 2 个**：`{CodeAdoptionRate:false, TodoAssistantDelegate:false}`。

也就是说这些开关的**主要来源是本地静态配置，不是云端下发**。实现时仍**只读不猜**，但基线应取 `product.json`（按 site 选对应 overlay：默认国际，另有 `product.{internal,ioa,cloudhosted,selfhosted}.json`），云端下发的少量 flag 覆盖其上。

---

## 7. 契约与注册表落点

### 7.1 `assets/contract/provider-registry.json`

照搬 `qoder_cosy` 形状，**site 不出现在契约里**：

- `optionSchemas` +1：`special.codebuddy_oauth.v1`，`fields: []`
- `families` +1：`family.codebuddy_oauth`，`label: "CodeBuddy"`，三个 surface（claude / codex / gemini），各 `defaultEnabled: true`，全部 scope 为 `bundle`
- `drivers` +1：`special.codebuddy_oauth`
  ```json
  {
    "driverId": "special.codebuddy_oauth",
    "driverContractRevision": 1,
    "upstreamProtocol": "special",
    "acceptedAuthSchemes": ["oauth"],
    "operations": { "forward": "supported", "test": "supported",
                    "discovery": "supported", "connectivity": "supported" },
    "capabilities": { "stream": true, "tools": true, "images": false },
    "outboundIdentityPolicy": { "kind": "managed_identity", "family": "codebuddy" },
    "optionSchemaId": "special.codebuddy_oauth.v1"
  }
  ```
  `capabilities.images` 先置 `false`；国际版目录含图像/视频模型，但在 U5/U11 确认 wire 形态前不开。
- `profiles` +3：`claude.codebuddy_oauth` / `codex.codebuddy_oauth` / `gemini.codebuddy_oauth`，`formComposition: "managed_account"`、`endpointPolicy: "fixed"`、`credentialPolicy: { mode: "managed_account", accountProviderType: "codebuddy_oauth" }`、`modelPolicy: "single"`、`allowedModelPolicies: ["single","passthrough"]`、`maturity: "experimental"`、`defaultUpstreamModel: ""`（理由见 §6.3）
- `conformance` +1：初始 `forward: "live_pending"`、`test: "live_pending"`、`discovery: "live_pending"`

### 7.2 注册表不变量

`src/domain/providers/registry.rs` 有硬编码计数，必须同步：

| 位置 | 现值 | 新值 |
|---|---|---|
| `:1001` `registry.profiles.len() != 76` | 76 | **79** |
| `:1457` `assert_eq!(registry.families.len(), 34)` | 34 | **35** |
| `:1458` `assert_eq!(registry.profiles.len(), 76)` | 76 | **79** |
| `:1406` `REVIEWED_FIRST_CLASS_PROFILE_ADDITIONS: [&str; 39]` | 39 | **42**（追加三个 profileId） |

（若曾按"拆两个 ProviderType"设计，这四处将变成 36 / 82 / 82 / 45，并多出一个 family、一个 driver、一份 option schema 与三个 profile —— 这是 §1.1 结论的量化代价。）

### 7.3 其他落点

- `src/domain/providers/model.rs`：`ProviderType` +1 变体及 `as_str()` / 反序列化分支
- `src/domain/codebuddy.rs`（新建）：站点、Profile、path 常量、identity 派生
- `src/proxy/codebuddy_runtime.rs`（新建）：`CodeBuddyRuntimeScope::derive(...)`（**site 进摘要**）、alias 表、capability 投影
- `src/api/accounts.rs`：三个控制面 handler
- `src/api/mod.rs`：三条路由 + 按 site 的默认值分叉（参照 `:5227-5239` qoder 的写法）
- `src/api/web/coverage.rs`：`serverCompatibilityProviderTypes` 追加 `codebuddy_oauth`
- `docs/README.md`：Provider 索引追加本文与 open-questions

---

## 8. 恢复与终态边界

与 `kimi_code` / `qoder_cosy` 完全一致，不放宽：

- 首次 eligible 401 **只允许刷新该绑定账号并重放一次**，且仅在下游提交前重放。
- 第二次 401、提交后错误、403、429、5xx、网络失败、generation drift、站点漂移，**全部是终态**。
- 流式在提交后出现的断流、重复或畸形 terminal 事件只能结束当前流，不能重放。
- 不存在跨账号、跨站点、跨 Provider 的 fallback。

### 8.1 上游错误的实际形态（U14，已关闭【D】）

| 情形 | 实测响应 |
|---|---|
| token 被篡改 / 缺失 | **HTTP 401，body 是 APISIX/openresty 的 HTML**，不是 JSON |
| 限流 | **HTTP 429**，`{"code":14003,"msg":"too many requests","requestId":"..."}` |
| 非流式请求 | HTTP 400，`{"code":11101,...}` |
| 首条非 system | HTTP 400，`{"code":11128,...}` |
| 模型未开通 | HTTP 400，`{"code":11102,"msg":"model [x] service info not found"}` |
| 模型无权限 | HTTP 400，`{"code":11133,"msg":"the request parameters were rejected by the model provider"}` |

> **实现约束**：401 的 body **不可假定为 JSON**。任何按 `code` 字段判定认证失败的逻辑必须先看 HTTP 状态码，解析失败时回落到状态码判定，否则会把 401 误分类。

### 8.2 授权终态码（自 bundle 提取【D】）

`toSignLicenseError` 的终态集合 —— 这三个码是**不可恢复**的，必须直接要求重新授权/联系管理员，不得重试：

| 码 | 名称 |
|---|---|
| 12005 | `LicenseSeatLimit`（席位超限） |
| 11212 | `LicenseExpired`（授权过期） |
| 11216 | `TrialExpired`（试用过期） |

限流/配额枚举（部分）：`14001 UsageLimitExceeded`、`14002 ConversationChatTooMany`、`14003 RateLimitError`、`14012–14018` 企业与用户配额系列、`10105 ConversationLimitExceeded`、`15001 WebSearchRateLimit`、`11115 ContextTooLong`、`6001–6008 CraftRate{TPS,TPM,TPH,TPD,RPS,RPM,RPH,RPD}Limit`。完整表见 `workbuddy-cliproxy/codebuddy-open-questions.md` §4.1。

**业务码定位算法**：官方 CLI 会**递归下钻最多 6 层**（`code` → `data` → `error`，并对 `message` / `details` 尝试 JSON 解析后再钻），只认 `code >= 1000`。反代若要向下游透传上游错误，需保持这层嵌套可被还原。

---

## 9. 分阶段实施与 live gate

| 阶段 | 内容 | 前置 |
|---|---|---|
| **P0** | 域模型、站点 Profile、identity 派生、契约与注册表落点、离线 fixture | 无 |
| **P1** | OAuth 三接口 + refresh（两站） | ~~U6、U7、U8~~ **已解除** |
| **P1.5** | 会话保活刷新扫描（规避 `12153`） | P1 落地后立即接入，见 §9.1 |
| **P2** | 数据面（Claude / Codex / Gemini 三 Surface，强制流式） | ~~U1、U2、U3、U9、U12~~ **已解除** |
| **P3** | `/v3/config` 权威目录 + capability 投影 | ~~U4、U5、U11~~ **已解除** |
| **P4** | 工具调用、thinking、usage、图像 | ~~U10、U11、U15~~ **已解除**（图像除外，见下） |

> **门槛状态（2026-08-31）**：U1–U15 已在国际版真实账号上全部关闭，采集结论见 `workbuddy-cliproxy/codebuddy-open-questions.md` §4。P1–P4 的未决项前置**全部解除**，可按序实施。
>
> 仍未覆盖、需在实施中另行处理的三项：
> 1. **国内站数据面无真实采集**，结论仅来自 `workbuddy-cliproxy` 源码。国内 site 的 `conformance` 应保持 `live_pending`，直到拿到国内账号。
> 2. **企业账号形态未测**（本次为个人账号 `enterpriseId: ""`），§6.4 的企业目录通道与 `X-Enterprise-Id` 头仍为未验证。
> 3. **图像/视频模型全部不可用**（`11102` 或 `Backend [aiart] is not supported`），P4 的图像部分无法在当前账号上验收。

~~**P2 之前不允许把国际站标记为可用。**~~ **该封锁已解除【D】**：U1 已关闭，国际站推理端点确认为 `POST https://www.codebuddy.ai/v2/chat/completions`，与国内 path 一致。现在改为**国内站**成为证据较弱的一侧 —— 国内数据面至今只有源码证据、无本仓库采集。

### 9.1 会话保活刷新（规避 `12153`）

**为什么必须单独立一阶段。** access token `expiresIn` 约 365 天【D】，所以"到期前 5 分钟刷新"（§4）在正常路径下**一年都不会触发一次刷新**。而上游会清理闲置的 refresh 会话：`workbuddy-switch` 记录过闲置数天后刷新返回 `12153 invalid_grant`（`Session doesn't have required client`）的真实事故【C】，该状态**不可自愈**，账号只能重新登录。

即：CodeBuddy 的 refresh 不只是"续期"，还是**会话心跳**。不主动刷新的账号会静默失效，而且失效点出现在用户下一次实际使用时，不是在后台。

> ⚠️ 命名避让：本仓库 `downstream_keepalive` / `record_responses_downstream_keepalive` 指的是 SSE 下行心跳，与此处无关。新增物一律用 `session_refresh_*` 命名，不要复用 `keepalive`。

**策略**

| 项 | 取值 | 理由 |
|---|---|---|
| 扫描间隔 | 24h | 对齐 `workbuddy-switch` 的每日一次；国际站清理阈值未实测，取已知安全值 |
| 首次延迟 | 服务启动后 5 分钟 | 与 `FIRST_HEALTH_CHECK_DELAY` 同风格，避开启动风暴 |
| 选取条件 | `provider_type == codebuddy` 且持有 refresh token 的**全部**账号，两站同等对待 | 不能按剩余有效期筛——`expiresIn` 一年，按期筛等于永不刷新 |
| 并发 | ≤2，账号间串行退避 | 刷新会轮换 refresh token，必须避免同账号并发；上游 429 为 `{"code":14003}`（§8.1） |
| 抖动 | 每账号 ±随机分钟 | 避免多账号在同一秒打上游 |

**复用现有机制，不新建**：调度器沿用 `src/api/provider_health_scheduler.rs` 的形态（常量化 `FIRST_*_DELAY` / `*_INTERVAL` / `MAX_CONCURRENT_*` + `tokio::spawn`）；单账号刷新走已有的 `account_refresh_plan` / `refresh_account` 路径（`src/api/accounts.rs`），保证与手动刷新同一把锁、同一套身份冲突校验（§4 的 `uid` 一致性检查、refresh token 轮换写回）。

**终态区分**（关键，不能只记"刷新失败"）

| 情况 | 处置 |
|---|---|
| `12153` / `invalid_grant` | **终态**。账号标记需重新登录并附原因，停止对该账号的后续保活扫描，控制面明确提示"会话已被上游清理，请重新登录"。**不重试** |
| 401 / 403 | 按 §8 走一次恢复；二次仍失败即终态 |
| `14003`（429） | 瞬时，退避后下一轮再试，不改账号状态 |
| 网络错误 / 5xx | 瞬时，不改账号状态，仅计数 |

失败原因必须与"token 自然过期"分开落库（两者的用户动作不同：前者只能重登，后者可自愈），并且**原因文本不得包含 token 或上游原始响应体**（沿用 `redact_provider_test_error` 的口径）。

**验收**（补入下方验收边界的离线项）

- 扫描只选中 codebuddy 账号，且不因 `expiresAt` 尚远而跳过。
- `12153` → 账号进入终态、不再被后续轮次选中、不产生重试。
- 429 → 不改账号状态，下一轮仍被选中。
- 刷新成功后新 refresh token 已写回；同账号并发刷新被锁串行化。
- 失败原因文本不含 token / 上游响应体。

**未决**：国际站的闲置清理阈值未实测（`12153` 是国内站的【C】级事故证据）。24h 是保守取值；若后续在国际站测得阈值，可据此放宽间隔。已登记在 `workbuddy-cliproxy/codebuddy-open-questions.md` §4.11。

### 验收边界

离线：站点 parse 与非法站点拒绝、identity 含 site 且跨站不碰撞、`X-No-*` 头齐备、cookie jar 复用、轮询 lease 串行化、flow TTL 与过期清理、refresh 身份冲突拒绝、header 最终覆盖、强制流式注入、目录权威/空/stale/协议漂移、alias 按 site 投影、未知模型 400、单次 401 恢复与二次 401 终态、Provider/Account/token 三代际漂移。

真实验收需要**国内与国际各一份**脱敏 receipt，覆盖：登录、refresh 轮换、`/v3/config` 目录、三 Surface 的非流与流式、tools、首个 401、第二个 401、429、中途断流、未知模型拒绝，以及日志/控制面/持久化文件不泄露 token。

国际 site 的采集已完成（2026-08-31，个人账号），可支撑 `fixture_verified`；**升 live verified 仍需在本仓库内按上述清单重跑一遍并留存脱敏 receipt** —— 本次采集是在 workbuddy-cliproxy 侧做的探针式验证，不是本仓库的验收流水线产物。

国内 site 至今**没有任何本仓库采集**，只能标 `live_pending`。

---

## 10. 订阅与计费接口（供应商节点"订阅信息"展示）

【D】2026-08-31 在国际站个人账号实测；路径与请求体来自 `workbuddy-switch`（面向 `www.codebuddy.cn` 的账号切换器）源码【C】，两站一致。

> 早前判断"CLI OAuth 凭据无法获取订阅信息"**已作废**——那次只做了 GET 扫描且未下探 `/meter/{action}` 层级。

### 10.1 端点

| 用途 | 方法 | 路径 |
|---|---|---|
| 订阅额度与到期 | `POST` | `{endpoint}/v2/billing/meter/get-user-resource` |
| 逐请求用量 | `POST` | `{endpoint}/billing/meter/get-user-request-usage` |
| 签到状态（**不实现**） | `POST` | `{endpoint}/v2/billing/meter/checkin-status` |

用量接口**没有 `/v2` 前缀**，加上会 404（`{"error_msg":"404 Route Not Found"}`）。鉴权仅需 `Authorization: Bearer {access_token}`，与数据面同一 token，无额外 scope。

### 10.2 订阅信息投影

请求：

```json
{"PageNumber":1,"PageSize":100,"ProductCode":"p_tcaca","Status":[0,3],
 "PackageEndTimeRangeBegin":"<now>","PackageEndTimeRangeEnd":"<now+N年>"}
```

响应为腾讯云计费信封 `data.Response.Data.{TotalCount,TotalDosage,Accounts[]}`。`Accounts[]` 是**资源包数组，不是账号数组**——个人 free 账号实测同时存在两个包：

| PackageName | CapacitySize | CapacityRemain | CycleEndTime |
|---|---|---|---|
| `Free Plan Subscription` | 100 | 100 | `2026-08-31 23:59:59` |
| `Bonus Pack` | 250 | 250 | `2026-09-14 14:15:28` |

节点展示字段映射：

| UI 字段 | 来源 | 备注 |
|---|---|---|
| 等级 | `PackageName`（细分用 `SubProductCode`） | 服务端给可读串，**不得客户端拼装 Free/Pro** |
| 到期 | `CycleEndTime` | `ExpiredTime` 实测为空串，不可用；也**不得**用 JWT `exp` 冒充 |
| 余额 / 配额 | `CapacityRemain` / `CapacitySize`，单位 `CapacityUnit` | 单位实测有 `credit` 与 `credits` 两种写法，比较前须归一 |
| 合计 | `TotalDosage` | 服务端已聚合，不要自行求和 |

**必须多包聚合**：按 `CycleEndTime` 升序展示，最近到期者优先。`Status` 请求参数 `[0,3]` 即"生效中 + 可用"。

### 10.3 用量投影

请求 `{"startTime":"...","endTime":"...","pageNum":1,"pageSize":3000}`，响应 `data.{total,data[]}`，行字段：

```
requestId, credit, model, client, requestTime, inputTrunc, input, agentPurpose
```

- **`input` / `inputTrunc` 是 prompt 正文**（CLI 请求实测为空，但字段存在）。消费方必须白名单裁剪，只取 `model` / `credit` / `requestTime` / `requestId`，**原文不得落盘、不得进日志、不得进 receipt**。
- 分页按 `total` 推进，需按 `requestId` 去重。
- 观测到 `codewise-model-a9` 等**后端真实模型 id**，与 CLI 别名不同，可用于交叉验证 §6 目录。
- 失败请求也可能计费：`gpt-5.6-terra` 返回 `11133` 拒绝，用量表仍记 0.02 credit。重试策略须据此设上限。

### 10.4 实现约束

- 归入**控制面只读查询**，不进数据面热路径；结果缓存，不得每次打开节点就打上游。
- 401 时走一次 refresh 重试再判终态，与 §4 同一策略。
- 敏感字段：`Accounts[].Uin` / `AppId` / `AccountId` / `DealName` / `ResourceId`、`AccountAttributes[].payerUin` 是真实腾讯云账号标识，**一律脱敏**。
- 签到（`checkin-*` / `daily-checkin`）属运营行为，且天然诱导多账号轮换，**本 Provider 不实现**，仅在此登记接口形态。

---

## 附：与 workbuddy-cliproxy 的差异清单

移植时以下几处**不能原样搬**：

| 项 | workbuddy-cliproxy 现状【C】 | 本文要求 |
|---|---|---|
| identity 派生 | `sha256(uid‖\0‖enterpriseId)[:12]`，不含 site | 必须含 site（§2.2） |
| `X-Auth-Refresh-Source` | 未发送 | 按官方发送 `plugin`（§4） |
| 站点 | `upstreamBase` / `originReferer` / `clientUA` 硬编码为国内 | 按 site profile 解析（§1.3） |
| 模型目录 | `models.yaml` 内嵌 16 个国内模型 | `/v3/config` 权威 + fail closed（§6） |
| 配置 | `pluginConfig { PromptRewrite, ModelManifest }`，无 site 字段 | site 属于账号 Profile，不是插件配置 |
| `forceMaxThinking` | 按 `hy3` 前缀硬编码 | 按 site × model 的 reviewed capability 表（§6.2） |

可以直接复用的：`cli-external-link` 时序、`X-No-*` 头集合、cookie 亲和的 login client、10 分钟 flow TTL、5 分钟 refresh 提前量、强制流式、`sanitizeBlockedTemplates` 的思路。

## 附二：`workbuddy-switch` 作为第二参照实现

`/data/projects/proxy/CodeBuddy/workbuddy-switch`（Tauri 账号切换器，面向 `www.codebuddy.cn`）是**纯控制面、无数据面**的独立国内实现，可交叉校验认证层。逐项对照见 `workbuddy-cliproxy/codebuddy-open-questions.md` §4.10。

**可借鉴**：

| 项 | 说明 |
|---|---|
| `build_auth_headers` 头集合 | `Authorization` + `X-User-Id`，**有企业 id 才**加 `X-Enterprise-Id` / `X-Tenant-Id`（同值），可选 `X-Domain`；印证 §5.2 不发 `Referer` / `Origin` |
| `X-Domain` 语义 | 取自 login 响应的 `domain`，值即站点 host（`www.codebuddy.cn` / `www.codebuddy.ai`）→ 可作 site 的服务端校验 |
| 401 内联 refresh 重试 | 一次调用内检测未授权 → refresh → 重放；控制面查询（§10）适用同一模式 |
| `needs_relogin` + 原因 | 把"刷新失败需重登"与"token 到期可自愈"分开落库 |
| 用量字段白名单裁剪 | `official_usage.rs` 显式声明 prompt / `input` 等字段永不复制；§10.3 采纳 |

**不可借鉴**：杀进程 + 改文件 + 重启的桌面式切换（`process.rs`）、自动签到与多账号积分轮换（运营行为，且诱导账号滥用）、`apiKeyHelper` / `settings.json.env.CODEBUDDY_AUTH_TOKEN` 注入路径（本 Provider 自己就是上游端点，不需要改 CLI 配置）。

**它帮不上的**：全部数据面结论（端点、强制流式、`11128` 首条 system、`reasoning_content` 非对称、dense delta、usage 字段、模型目录）——它一行都没有。
