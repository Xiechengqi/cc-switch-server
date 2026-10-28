# CodeBuddy OAuth 单账号反代（实现合同）

> **状态：本地实现完成，`fixture_verified / live_pending`。** 当前实现包含 `codebuddy_oauth` ProviderType、双站点账号域模型、OAuth/refresh、三 Surface driver、严格流终态、同账号恢复、目录、额度、管理 API 与 Web 入口。本文是这些本地实现的权威合同；后文保留的“新增/实现时”措辞记录设计落点，不表示对应代码仍缺失。仓库尚无 Intl/CN 两站真实订阅的脱敏 receipt，因此不得写成 `live_verified` 或发布级真站兼容。
>
> 本文只写**已证实**的部分。所有未决项、以及它们各自的判定标准与抓包方法，见 workbuddy-cliproxy 仓库的 `codebuddy-open-questions.md`，编号 `U1`–`U15` 在两份文档间通用。
>
> **2026-08-31 更新：U1–U15 已在国际版真实账号上全部关闭**（CLI v2.142.0 + mitmproxy 抓包）。本文相关章节已回填实测结论，标注 **【D】**。仍未覆盖的三项（国内站数据面、企业账号、图像模型）见 §9。

本文定义 cc-switch-server 的 `codebuddy_oauth` Provider 边界。一个 CodeBuddy Provider 固定绑定一个 CodeBuddy Account；Server 不实现账号池、轮询、权重、按配额/并发选号、自动切站，或跨 Provider fallback。

当前 `special.codebuddy_oauth` Driver contract revision 为 **2**。revision 2 增加 CodeBuddy 专属的空消息、空 delta 和 tools/tool_choice 线格式规范化；2026-09-18 增量复核又关闭了中断工具轮次、reasoning-only 历史和取消生命周期的离线缺口，2026-09-19 再完成三 Surface、双站恢复共用的请求生命周期内存预算。`assets/contract/codebuddy-reference-delta.json` 现为 append-only schema v2：原 11 个 v1 字段由独立 digest 固定，15 条不可变 observation 将 CB-01 至 CB-04、CB-N1 至 CB-N5、CORE-N1/N2、LIVE-N1 和三条拒绝边界映射到外部只读提交及本仓库 committed target object。

---

## 0. 证据来源与置信度标注

| 代号 | 来源 | 采集方式 | 时间 |
|---|---|---|---|
| **A** | CodeBuddy CLI npm 包：`dist/codebuddy.js`（约 25 MB bundle）、`product.json`、`product.internal.json` 等 overlay | `npm pack` 解包后静态阅读 | 2026-08-31 |
| **B** | 国际版官方文档 `https://www.codebuddy.ai/docs/zh/cli/*` | `curl`（`WebFetch` 对该域名被拦截） | 2026-08-31 |
| **C** | `/data/projects/proxy/workbuddy-cliproxy` 生产实现（国内站已跑通） | 直读源码 | — |
| **D** | **国际版真实账号抓包**：CLI v2.142.0 + mitmproxy 11.0.2 正向代理，个人账号（`enterpriseId: ""`） | 实测流量 + bundle 定点反查 | 2026-08-31 |
| **E** | `/data/projects/proxy/CodeBuddy/cli2api@624874a0331f5e8f012ef104b633e69826487458` | 只读 `git show`；排除工作树中两个未跟踪文件 | 2026-09-18 |

正文中的标注含义：

- **【D】** — **真实流量实测**，置信度最高。脱敏证据见 `workbuddy-cliproxy/codebuddy-open-questions.md` §4 与 `~/cbcap/redacted/`。
- **【A】/【C】** — 代码直读或双证，可直接实现。
- **【E】** — 成熟反代的已提交对象，只作增量交叉证据；不覆盖官方 CLI/真实流量，也不进入构建或运行时依赖。
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

`domain` **不是 principal**，而是同一 site 内可轮换的路由属性。refresh 返回 `codebuddy.ai ↔ workbuddy.ai` 或 `codebuddy.cn ↔ workbuddy.cn` 时更新 Profile，但不推进 `auth_identity_generation`；跨 Intl/CN 的 domain 仍立即拒绝。历史账号不批量改 ID，重新登录时按 `site + uid + enterprise_id` 查找并复用原本地 ID；只有找不到历史账号的新登录才生成 domain-free v2 ID，避免打断既有 Provider binding。

本实现只接受个人订阅 rail。新 OAuth 登录闭合 `/v3/config` 身份后若 `enterpriseId` 非空即 fail closed，不创建账号；既有企业 fixture 只保留可读兼容，不构成企业反代承诺。

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
- 容量有界：同一管理员 principal 最多 4 个 CodeBuddy flow，CodeBuddy flow store 全局最多 64 个，通用 device-flow principal binding 全局最多 256 个；满载返回 429，过期项先清理，部分写入失败会回滚 principal binding。
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

在注入固定非空 system 前，revision 2 会删除 content 缺失、null、空白字符串或空数组且不承载协议语义的 message，避免上游 `11151`。assistant 的有效 `tool_calls` 与带非空 `tool_call_id` 的 tool result 即使 content 为空也必须保留；清理后为空时只注入固定最小 system，不改写普通 prompt。该差分来自只读 `cli2api@e5893f0`，目标实现采用更严格的 tool-result 保留边界。

2026-09-18 的 CB-N1 在这一步之前增加 CodeBuddy 专属历史 repair【E】：只有 assistant call ID 唯一、function name 合法、arguments 是完整 JSON，且紧随其后的 tool result ID 与 call 集合一一对应时，才原样保留完整轮次。缺 result、孤立 result、重复或错配 ID、截断 arguments 都只从历史中删除不可发送的协议片段；assistant 若仍有正文或 reasoning 则保留。实现不合成 tool result、不修改用户 tool output，也不把任何失败隐藏成成功，用于消除下一轮可复现的 `11148 tool_call_sequence_broken`。

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
- 下游 canonical `reasoning.effort` 会归一为顶层 `reasoning_effort`；同时出现且值冲突、类型错误、或 effort 不在实时目录的 `supportedEfforts` 时，本地 400。仅在下游明确请求 reasoning 且未指定 effort 时采用目录非空 `defaultEffort`，该字段缺失、`null` 或空白时回退到非空 `reasoning.effort`；不会无请求自动开启推理。显式非字符串 `defaultEffort` 不通过这条回退获得授权。
- OpenAI named `tool_choice` 会先确认目标函数存在，再把 `tools` 缩减到该函数并映射为上游支持的 `"required"`；缺失/未知函数或畸形类型在发网前失败。
- `tools: null`/`[]` 会与失去依附的 `tool_choice` 一起删除；`tool_choice: none` 同步删除 tools，`auto`/`required` 仅在非空 tools 存在时保留。named function 继续执行名字格式和声明存在性校验，不能借清理降级为 auto。
- `stream_options.include_usage=true` 是末帧 usage 的来源；实现若需用量统计必须带上。
- 回填轮的 assistant / tool 消息带非标准 `messageId`、`model`、`requestModelId`、`requestModelName`、`traceId`、`conversationRequestId`、`agent`。这些**不是必需的**，反代可不透传。

**响应 SSE**：`object: "chat.completion.chunk"`，delta 为**稠密**结构，每帧都带全部键 `content` / `reasoning_content` / `function_call` / `refusal` / `tool_calls` / `extra_fields`。

> **关键不对称【D】**：推理内容在响应里叫 **`delta.reasoning_content`**，但下一轮请求的 assistant 消息里叫 **`reasoning`**。做多轮工具循环的实现必须完成这层改名，否则思维链在回填时丢失。

CB-N2 现在先把 assistant 的非空 `reasoning_content` 规范化为 `reasoning`，再做空消息过滤，因此 reasoning-only 历史不会被误删。Responses 的相邻 reasoning/message/function call 先由共享 transform 合并为一个逻辑 assistant turn，再统一执行一次 CodeBuddy history repair；Claude Messages 与 Chat Completions 也走同一 canonical Chat 边界。

CB-N3 的差分结论是“共享 bridge 已覆盖”，没有增加 Provider 私有名字猜测。Responses namespace 声明会生成稳定、最长 64 byte 的完整扁平名，声明、历史 call、result 与返回恢复共用同一 request-local context；Claude/Chat 中本来就合法的完整名和普通 `user__literal` 名保持不变。CodeBuddy 不采用 Kiro 的唯一裸 child fallback，不能依据历史、Account、Share 或 session 猜 namespace。

`tool_calls` 为标准增量（首帧 `id`+`name`，后续帧只递增 `arguments`）；**首帧的 `index` 是否存在随模型而变**，按 `index` 归并的实现须容忍首帧缺失。终帧 `finish_reason` ∈ `{tool_calls, stop, length}`，随后独立一帧携带 `usage`，最后 `data: [DONE]`。**U10 已关闭。**

缺失工具 `index` 时的 ID 找回使用上游完整 ID 字符串，仅把全空白 ID 当作无 ID；不能先裁剪再查找。并行工具分片顺序变化时，`" call-b "` 必须命中同一个已聚合工具，不能因查找用了 `"call-b"` 而误落到数组位置对应的另一个工具。此项只修目标 Server 的本地身份保真，不推出厂商新的工具 ID 格式。

显式 `delta.tool_calls` 在流式清洗与非流聚合两条路径都必须是数组或 `null`：`null`、空数组继续作为空占位清理，合法数组继续按工具 ID/index 聚合；字符串、对象、数字或布尔值不能因“空值清理”被吞掉，也不能原样转发后由 `finish_reason`/`[DONE]` 伪装成成功。这是本地响应形状门槛，不推断新的 WorkBuddy wire。

SSE 解码接受 CRLF/LF/单 CR event boundary、多条 `data:`（按规范用换行拼接）、comment、`event:`、`id:` 与 `retry:` 心跳字段；仍要求唯一 `[DONE]` 后到达 EOF 才提交终态，`[DONE]` 后数据、重复 terminal、缺 terminal 或超限 event 全部失败。

流式 decoder 与非流 Chat 聚合器在任何 `push` 错误后锁存失败，后续空块、其他数据和 `finish` 均不能把已拒绝的响应改判为成功；decoder 的 `finish` 错误同样不可由后续数据补救。首次错误保留原上游/本地协议或容量分类，之后只返回固定的本地协议错误。该规则是目标 Server 自己的 EOF 成功门槛，不从参考项目在首个 `[DONE]` 停止扫描的聚合行为推断新厂商 wire。

目标解码器还按最早的完整空行识别混合 `LF+CRLF`、`CRLF+LF` 等事件边界，不能因为同批里较晚的标准边界把先到事件与 `[DONE]` 合并。逐字节分块和 Intl/CN × 三 Surface × 流/非流本地回环已覆盖；这只修本地 SSE 解析，不证明厂商真实发过混合行结束符，也不改变唯一 terminal+EOF 门槛。

`finish_reason` 已出现但 `[DONE]` 尚未来到的窗口只允许独立 usage 尾帧或无语义心跳；新 content/reasoning/tool 等业务 choice 在解码与非流聚合入口均失败关闭，不能被后续 `[DONE]` 伪装为成功。Intl/CN 的流式回环验证此时只输出失败终态且不触发账号刷新。此限制不要求 `[DONE]` 前必须有 `finish_reason`，以保留既有 liveness 兼容样本；厂商真实终态接受性仍待两站 receipt。

显式非零业务错误码优先于同帧 `choices`：即使错误 envelope 混入空 choices 或看似完整的 content/finish，decoder 与直接非流聚合器都不得把它当作普通 Chat chunk。已知 `14003` 仍归类限流，不因混合帧误记成功、转成普通 502 或触发账号刷新；`code=0` 正常业务帧保持原有处理。此项是本地错误优先级门槛，不推断厂商确实发送混合帧。

CB-N5 已用 CodeBuddy loopback fixture 覆盖 `[DONE]` 前截断、重复/后置数据、`[DONE]` 后无 EOF，以及下游在 marker 前后取消。共享 stream guard 会关闭唯一上游 body 并释放 Account/Share 租约；专项差分为绿，因此没有复制 cli2api 的语言特定取消分支，也没有放宽“terminal + EOF”规则。

revision 2 在 CodeBuddy decoder 内删除空 `content`、`reasoning_content`、`reasoning`、`refusal`、空 `tool_calls` 与 dummy `function_call`，重复 role 只保留第一次；若 chunk 随后不含 role、finish、usage、error、tool、reasoning 或 content 语义则整帧丢弃。文本字段只删除真正的空字符串或 null，单独成帧的空格必须保留，以免 `"left" + " " + "right"` 被拼成 `"leftright"`；显式非字符串文本按本地协议错误拒绝。finish、显式 usage（包括零）、真实 tool arguments 和合法首 role 不得丢失。该差分以 `cli2api@32aa108` 及已提交 `sse.go::dropEmptyString` 为只读交叉证据，严格 `[DONE]` + EOF 规则不变。

旧式 `delta.function_call` 只有 `null` 或空占位可由上述清洗规则删除。非空调用在流式 decoder 与非流聚合器均固定 502 拒绝：当前三 Surface 的非流聚合只表达 `tool_calls`，不能让旧式调用在聚合时消失。此为本地失败关闭门槛，不表示两站实际发送该字段；正常 `tool_calls` 路径和 dummy 清洗不变。

### 5.7 usage 字段

末帧 `usage` 共 13 个字段【D】：

```
prompt_tokens, completion_tokens, total_tokens, credit,
completion_thinking_tokens, completion_tokens_details{...}, prompt_tokens_details{...},
cached_tokens, cache_read_input_tokens, cache_creation_input_tokens,
prompt_cache_hit_tokens, prompt_cache_miss_tokens, prompt_cache_write_tokens
```

`credit` 是计费信号，`completion_thinking_tokens` 单独计推理 token。**U15 已关闭。**

CB-N6 对缓存 usage 再做一层守恒归一化。read 固定按顶层 `cache_read_tokens` → `prompt_tokens_details.cached_tokens` / `input_tokens_details.cached_tokens` → vendor legacy 字段取第一个有效非负整数，write 同理先取顶层 `cache_write_tokens`，再取 nested creation/write，最后取 legacy。显式 `0` 是权威值，不允许被后面的非零 alias 覆盖；负数、小数和越界整数不进入 token 计量。Claude Messages 将 inclusive prompt 拆成 fresh/read/write；Codex Responses 保持 inclusive `input_tokens` 并在 details 写 read/write；Gemini 保持 inclusive prompt 和 cached count。三类 Surface 的 total 都按 inclusive input/output 守恒，不重复加入 cache token。

流式和非流式入口现在对 `usage` 使用同一外层形状边界：显式非 `null` 值必须是对象，数字、字符串和数组在下游成功提交前按本地协议错误拒绝。`usage:null` 视为缺失并从规范化流帧移除；合法对象（包括显式零）与原有 cache 字段优先级不变。此项不推断新的上游计费字段或真实厂商接受性。

独立 usage 尾帧可以省略 `choices`，也可以显式写 `choices:[]`：流式 decoder 早已接受前一种形状，非流 Chat 聚合现在与之保持一致，并保留 cache read/write 与 total。只有 `usage` 通过对象校验时才允许缺省 `choices`；显式非数组 `choices` 或既无 `choices` 也无 `usage` 的非流帧仍拒绝。该兼容形状来自已提交 `cli2api` WorkBuddy 聚合器的可选 `Choices` 字段与本仓库流/非流差分，不把参考的宽松首个 `[DONE]` 行为迁入唯一终态 + EOF 合同；Intl/CN 真实 receipt 仍待证。

### 5.8 Provider lifecycle facade

CORE-N1 将精确 `codebuddy_oauth` Account binding、canonical request/model、站点绑定的 live catalog capability、payload preparation 和 Provider/Account generation fence 收敛到 `src/proxy/providers/codebuddy/`。共享 `forwarder.rs` 仍拥有 Share/Account lease、usage、terminal、统一 attempt budget，以及是否允许原账号在下游提交前执行一次 401 refresh/replay。此次拆分不改变 endpoint、header、payload bytes、错误记录分支、恢复次数或 terminal + EOF 合同，也不引入 Account、Provider、site、domain 或 rail fallback。

EVID-N1 冻结 `d81056c` 中原 schema-v1 文件及全部 11 个历史字段，追加 `cli2api@624874a` / tree `c2f02a3` 的 committed-object snapshot，并明确排除参考工作树中的 `proxy.html`、`proxy.md`。首轮 15 条 observation 分别记录 differential、live gate 或 reject，绑定 source path/symbol/digest、本仓库 baseline/implementation commit/tree、合同锚点和 fixture；默认 audit 完全自包含，只有显式 `node scripts/audit/audit-codebuddy-reference-delta.mjs --check-sources` 才读取外部已提交对象。CB-R1～R3 拒绝账号调度与跨边界 fallback、运营/商业控制面，以及未验证能力、伪 live 和弱化 terminal + EOF。

### 5.9 请求生命周期内存预算

CORE-N2 只对精确的 `codebuddy_oauth` Provider 启用；普通 `claude`、`claude_auth`、`gemini_cli` 或同一 App 下的其他 Provider 不会被误启用。Claude Messages、Codex Chat Completions/Responses 与 Gemini GenerateContent 的流式、非流式路径共用同一套容量语义。Intl/CN 同账号在下游提交前执行一次 401 refresh/replay 时继续使用原 request budget，不能通过恢复动作重置已经消费或已经耗尽的预算。

核算范围包括 raw/decoded/canonical body、runtime/catalog、prepared payload、JSON wire、请求 ID、目标 headers、错误响应与解析工作集；流式路径还覆盖 transport chunk、rolling decoder buffer、event parse、canonical `Bytes` owner，以及 Claude/Codex/Gemini 下游 bridge retained state。非流路径的 content、reasoning、tool-call arguments 和最终 JSON `Value` 也在同一预算内；reservation 跟随最终 owner，到最后一个响应或 chunk owner drop 才释放。

容量耗尽是 sticky 的：未提交响应返回稳定 `503`、`Retry-After: 1` 与 `cc_switch_request_memory_exhausted`；已提交 200 的 stream 只输出一个不含预算内部细节的 terminal。当前上游被取消，不 refresh/replay，不切 Account、Provider 或 site；usage 记录 `memory_capacity`，Provider outcome 归为 `CapacityShed` 而不是普通 `NetworkFailure`。分发边界对 `forward_codebuddy` future 做堆化；非流完成上游聚合后的收尾再经独立 Tokio 任务切断 `forward_with_attempt` 到 usage durable commit 的深 poll 调用栈，正常路径仍等待收尾结果，调用方取消时由既有 abort guard 中止任务并释放 Account/Share guard。CodeBuddy Provider 三 Surface 自测和完整 library 在默认 2 MiB Rust 测试线程栈下通过；完整默认栈 API 合同另有 Claude/Kiro 用例栈溢出，不据此声称全仓默认栈已清零。此项只修本地执行栈，不改变 WorkBuddy wire 或双站验收状态。

`CB-OBS-0015` 只把 `cli2api` 的 1/16 MiB body 读取上限、16 MiB 单条 SSE line ceiling 和 retained non-stream aggregation 当作差分信号；参考项目没有证明统一生命周期预算、payload/header 副本、下游 transform、容量分类或上述恢复语义。`CORE-N2-CODEBUDDY=fixture_verified` 绑定本仓库 `573dc47` 的独立实现和 66 个 CodeBuddy、26 个 request-memory、15 个 memory-exhaustion 专项测试。它不构成 Intl/CN 真实长流或内存压力 receipt，两站仍分别为 `null/live_pending`。

### 5.10 第二轮 cache usage 与共享 Responses 复核

2026-09-28 的 committed-object snapshot 冻结 `cli2api@34c91b899aa2289f5ee49618d2d300f3eeaffedb` / tree `a0a1a3e6530e160284dba8eb18b85555ceab6a10`，仍排除未跟踪的 `proxy.html`、`proxy.md`。`a9da609` 与 `fff3419` 暴露的 cache usage 丢失由 CB-N6 独立修复：decoder 和非流 aggregator 先规范化 usage，内部日志与三个下游 Surface 使用同一字段优先级；CodeBuddy Chat→Claude stream 持续增量发送工具参数、捕获独立 usage 尾帧，并只在唯一 `[DONE]` 与 EOF 都验证后提交成功终态。Intl/CN × 三 Surface × stream/non-stream 的 loopback fixture 同时跑非零与显式零场景，覆盖 read/write、冲突字段与 total 守恒；另有 usage-tail 截断 fixture 证明不会先成功后报错，状态为 `fixture_verified`。

同一轮复核确认 `f44e887` 的 `finish_reason=length`、`b415dd0` 的 namespace identity 和 `9f9b66e` 的 custom tool 已由共享 Responses bridge 的流/非流 fixture 覆盖，无需增加 Provider 私有状态机。`aeaa4ac` 对已完成 malformed function call/result 的静默丢弃不采纳；本仓库继续在任何 done/completed 前 fail closed，不把坏 arguments 改成 `{}`，也不隐藏损坏数据。

reference delta 现在保留原 15 条 observation 并追加 `CB-OBS-0016`、CB-N6 enhancement 与独立 second-round review；默认 audit 不读取外部仓库，`--check-sources` 才验证新旧 committed objects。该结果不提升 Intl/CN receipt，也不开放 CB-N4、企业或多模态能力。

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

- 目录按 App、Provider revision/runtime fingerprint、Account、identity generation、token generation、**site**、进程内账号实例及实际请求身份摘要隔离，并 single-flight 获取。摘要覆盖凭据、profile、有效期与测试端点；不会将明文放入缓存键。同代次凭据/UID 改变或同 ID 账号重建不复用旧目录，完全相同作用域的 stale fallback 规则不变。返回目录和发起 generation 前再复核当前绑定及请求身份；分次复核不是跨存储原子事务。
- **成功空目录是权威结果**，保持为空，不与 Provider 配置、静态 alias 或其他账号目录合并。
- 只有 network / 408 / 429 / 5xx 才允许读取**完全相同作用域内**、24 小时硬上限内的 stale cache。
- 认证错误、超限响应、坏 JSON、未知成功结构、身份 scope 漂移，以及"上游非空但全部模型均不在 reviewed allowlist"，一律 fail closed 并清理当前 scope。
- `/v3/config` 若出现业务 `code`，必须可解析且为 0；无法解析的字符串、`null`、布尔或对象不能当作缺失后继续接受 `models`。模型 `id` 保留上游原样，只允许与 reviewed allowlist **精确相等**的启用项进入公开目录；首尾空白 ID 失败关闭，大小写不同的 ID 不会被强制转小写后冒充已审核模型。
- **运行时不存在静态 entitlement fallback** —— 与 `kimi_code` 条款一致。`product.json` 内嵌目录只作为**字段补全的元数据源**（`mergeModelsById` 的 local 侧），不作为 entitlement 来源。这个区分是本 Provider 能通过该条款的原因。
- `/v1/models` 只在当前 Share 选中的 CodeBuddy Provider 范围内，公开上游目录与 reviewed allowlist 的交集，不参与 Provider 或账号选择。
- live catalog 出现启用但未 reviewed 的模型时只输出不含凭据/请求内容的结构化漂移告警（site、模型计数、固定 client version），不会自动放行新模型。

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

### 6.4 企业目录（本期明确不实现）

```
GET {endpoint}/console/enterprises/{enterpriseId}/config/models
```

`ModelsProductProvider`【A】。`enterpriseId` 缺失时直接跳过 —— **个人账号永远不触发**。当前登录入口对非空 `enterpriseId` 明确拒绝，不会误把企业账号送入个人订阅 rail；只有未来单独立项并取得企业真实证据后才可实现该目录。

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
    "driverContractRevision": 2,
    "upstreamProtocol": "special",
    "acceptedAuthSchemes": ["oauth"],
    "operations": { "forward": "supported", "test": "supported",
                    "discovery": "supported", "connectivity": "supported" },
    "capabilities": { "stream": true, "tools": true, "images": false },
    "outboundIdentityPolicy": { "kind": "managed_identity", "family": "codebuddy" },
    "optionSchemaId": "special.codebuddy_oauth.v1"
  }
  ```
`capabilities.images` 保持 `false`；国际版目录含图像/视频模型，但在 U5/U11 确认 wire 形态前不开。
- `profiles` +3：`claude.codebuddy_oauth` / `codex.codebuddy_oauth` / `gemini.codebuddy_oauth`，`formComposition: "managed_account"`、`endpointPolicy: "fixed"`、`credentialPolicy: { mode: "managed_account", accountProviderType: "codebuddy_oauth" }`、`modelPolicy: "single"`、`allowedModelPolicies: ["single","passthrough"]`、`maturity: "experimental"`、`defaultUpstreamModel: "auto"`。`auto` 只是表单必需的站点中立 sentinel，运行时必须按绑定 Account 的 site 解析，绝不能直接发往上游。
- Registry conformance 当前三项均为 `fixture_verified`，只表达本地实现与 fixture 等级；独立的 `assets/contract/provider-conformance-evidence.json` 将三项 live gate 保持为 `live_pending`，receipt 为空。两类状态不得再写回同一字段或相互冒充。

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

数据面的同账号恢复只针对没有已识别业务拒绝码的 HTTP 401（包括 APISIX HTML 401），且至多一次。HTTP 401 即使夹带已知 `14003` 限流码，也按限流终止当前请求；上表三项许可码即使映射为下游 401，也不得刷新或重放。`12153` 是需要重新登录的会话终态，在生成响应中出现时同样不触发刷新重放；HTTP 403 不因业务码映射为 401 而升级为可恢复。未识别数字码与无业务码的 HTTP 401 仅在**恢复资格判断**上仍按原 HTTP 状态回退；下游状态映射不变。Intl/CN × 三 Surface × 流/非流的本地回环覆盖已知终态，不能据此声称厂商实际会在生成响应中返回这些组合。

限流/配额枚举（部分）：`14001 UsageLimitExceeded`、`14002 ConversationChatTooMany`、`14003 RateLimitError`、`14012–14018` 企业与用户配额系列、`10105 ConversationLimitExceeded`、`15001 WebSearchRateLimit`、`11115 ContextTooLong`、`6001–6008 CraftRate{TPS,TPM,TPH,TPD,RPS,RPM,RPH,RPD}Limit`。完整表见 `workbuddy-cliproxy/codebuddy-open-questions.md` §4.1。

**业务码定位算法**：官方 CLI 会**递归下钻最多 6 层**（`code` → `data` → `error`，并对 `message` / `details` 尝试 JSON 解析后再钻），只认 `code >= 1000`。反代若要向下游透传上游错误，需保持这层嵌套可被还原。

目标 Server 保留上述递归识别用于内部分类，但对外错误文本只显示本地已映射为认证、权限、限额或请求错误的业务码；未知数字码仍按上游错误处理，不回显其原值。HTTP 与 SSE 错误消息先经通用 Bearer/token/API key 等敏感字段脱敏、控制字符过滤和长度限制，才进入下游错误及失败记录。此为本地安全边界，不代表厂商只会返回已映射的错误码，也不能识别未带敏感字段线索的任意秘密。

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

离线：站点 parse 与非法站点拒绝、identity 含 site 且跨站不碰撞、同站 domain 轮换不改 identity 与旧 ID 惰性复用、个人 rail 拒绝企业 identity、`X-No-*` 头齐备、cookie jar 复用、轮询 lease/容量/TTL/过期清理、refresh 身份冲突拒绝、header 最终覆盖、强制流式注入、named tool choice 与 reasoning effort 校验、完整/中断/孤立/重复/截断工具轮次、reasoning-only 与三 Surface namespace 闭环、SSE 多行/心跳/唯一终态、终态前后取消和租约释放、目录权威/空/stale/协议漂移、alias 按 site 投影、modern billing 合并与旧接口回退、官方用量分页/去重/prompt 不落盘、未知模型 400、单次 401 恢复与二次 401 终态、Provider/Account/token 三代际漂移。

真实验收需要**国内与国际各一份**脱敏 receipt，覆盖：登录、refresh 轮换、`/v3/config` 目录、三 Surface 的非流与流式、tools、首个 401、第二个 401、429、中途断流、未知模型拒绝，以及日志/控制面/持久化文件不泄露 token。

LIVE-N1 的 `scripts/smoke/codebuddy-real-receipt.mjs` 一次只接受 `--site intl` 或 `--site cn`。对应 Account、Claude/Codex/Gemini 三个 Provider、Share、exact model 和 receipt path 必须使用该 site 的独立环境变量；控制面只对通过生产 `CodeBuddyAccountProfile::parse` 验证的 Account 发布脱敏 `codebuddyCredentialScope.site`，validator 在开始和落盘前均要求该站点与 `--site` 精确相等，缺字段或漂移时失败关闭。投影不暴露 profile domain、UID、enterprise ID 或 token；脚本选择的站点及 receipt 自述不能代替真实 Account profile。三个 Provider 都必须是 `special.codebuddy_oauth` / `ready`，固定同一 Account 与当前 `authIdentityGeneration`，Share 显式固定三项绑定。validator 重新读取三份 `source=codebuddy_live_model_catalog`、`stale=false` 的 fresh catalog，并把 target commit/harness、Account 两代际、三个 Provider revision/runtime digest、Share revision/binding digest、exact model、三份 catalog snapshot 与六个 non-stream/stream body hash共同绑定进 scope digest。

Share 控制面投影中的 `bindings` 已包含 `share.app` 的主绑定，主字段不是额外的第四条绑定。validator 直接要求 Claude/Codex/Gemini 各恰好一条完整匹配的绑定，并要求 `share.app/providerId/providerType` 与其对应条目一致；同 App 即使另有一条正确绑定，只要还存在重复或冲突条目便拒绝。此项防止歧义 Share 被写进真实验收证据，不改变生产 Share 路由或声称跨 API 原子快照。

三个 Provider 都按 Server 真实 `ProviderView` 校验顶层 `revision` 与 runtime `providerRevision`：两者必须明确存在、是非负安全整数且相等，runtime `runtimeFingerprint` 必须是非空白字符串；Share `configRevision` 也必须明确为非负安全整数。缺失、不一致、仅有旧 mock 的虚构顶层 `providerRevision` 或空白 fingerprint，在三份目录请求前失败；不以 `0` 补齐 receipt 作用域。此检查只加固私有证据，不能替代 Intl/CN 各自的真实上游验收。

Intl/CN 的 Account 投影还必须明确给出布尔 `hasApiKey=false` 与 `needsRelogin=false`。字段缺失、`null` 或字符串 `"false"` 不等同于布尔值，均在 Provider 和三份目录请求前拒绝；这不改变生产 OAuth 身份或站点绑定。

Intl/CN 两站的 validator 还要求 Share 明确给出 `enabled=true` 和 `status=active`；缺字段、disabled 或 paused 都在目录请求前失败关闭。仅因字段缺失而没有返回 `false`/非 active 值，不能被推断为已启用。此检查仍只约束私有验收证据，不把控制面分次读取变成原子快照。

三份目录请求各带 `app`、所选 `expectedShareId` 和该 App 的 `expectedProviderId`。Server 的签名 Router ingress 先按实际 Share/App 选择 Provider，再比对预期绑定；普通 `providerId` 查询不承担选择作用。此核对只证明验收目录的路由作用域，不证明厂商模型权益或目录与后续推理组成原子快照。

Intl/CN 私有 receipt 在末次 Account、三 Provider 与 Share 绑定复核后，还会通过同一签名 Share/各 App Provider 栅栏重新读取 Claude、Codex、Gemini 三份目录。各目录必须继续是合法实时来源、非 stale；完整 `data` 的所有模型 ID 必须是非空字符串、无首尾空白且唯一，选定模型须恰好一次。完整 `data` 按 ID 确定性排序后计算摘要，末次摘要分别等于 receipt 所绑定的初次摘要；纯行重排不算漂移，能力字段等内容变化仍拒绝。初次与末次共六份目录时间戳在同一接受时刻复核新鲜性。撤下或添加模型、目录失效、来源漂移及超前时间均拒绝接受。这是私有证据门槛；分次读取不是跨控制面与厂商的原子快照，也不能代替双站真实账号验收。

私有 receipt schema v2 必须完整提供 16 项 `pass`、规定的 measurements、固定恢复决策、decoy 零计数与 secret scan；真实文件位于仓库外且权限为 `0600`。缺输入时只输出 `blocked_inputs/live_pending`；`CC_SWITCH_CODEBUDDY_HARNESS_MODE=fixture` 即使所有 mock 检查通过也只接受 `contract_verified/live_pending`，不能写成 `live_verified`。真实模式只有完整私有 harness 已覆盖登录、refresh rotation、`12153`、权威空目录、billing、三 Surface、两段 401、唯一 terminal + EOF、generation/decoy/secret 检查时才接受 `live_verified`。任一站成功不得继承给另一站，receipt 也不自动开放 `deepseek-v4.1-flash`、企业或多模态能力。

validator 在控制面请求前由只读描述符限量读取私有 receipt；读取前后都核对同一文件描述符的设备、inode、链接数、大小、权限以及纳秒级修改/状态变更时间。普通原位改写即使保持文件大小且发生在最后一次读取后、EOF 检查前，也会失败关闭，不再把读取期间变化的文件当成稳定证据。这是文件快照门槛，不证明并发原位改写与元数据复原情况下的原子视图，也不证明部署 commit 与本地 HEAD 一致。

最终接受前还会重新解析配置的 receipt 路径，并把当前文件的设备、inode、链接数、大小、权限和纳秒级 mtime/ctime 与读取时的快照比较。若控制面校验期间路径被替换，或原文件被普通原位改写，即使原先读入的内容有效也拒绝该站。末次文件检查不是与控制面或后续文件操作绑定的原子事务，恶意元数据复原和检查后的变化仍需独立防护。

末次检查还会通过 `O_NOFOLLOW` 只读描述符重新读取不超过 1 MiB 的完整 receipt 字节（含 EOF），与先前验收的原始字节逐字节比较，并在读前、读后及路径处复核文件身份/元数据。Intl/CN 的对抗 fixture 在控制面请求期间等长改写文件、对末次元数据读取返回原值时，旧流程会错误接受；现在两站均脱敏拒绝。fixture 的元数据伪装仅用于证明不能只依赖元数据，不代表普通用户能恢复真实 `ctime`；末次检查之后的并发变化和跨 API 原子性仍不由此保证，也不提升真实订阅验收状态。

receipt 的 `recordedAt` 必须在最终接受时仍位于 24 小时有效期内（未来时钟偏差最多五分钟）。validator 在内容校验时和末次 Account/Provider/Share 绑定复核后各检查一次；若复核期间过期，则拒绝该站的 receipt。此规则只约束私有证据时效，不修改生产目录 TTL 或 CodeBuddy wire；末次控制面读取并非跨 API 原子事务。

国际 site 的采集已完成（2026-08-31，个人账号），可支撑 `fixture_verified`；**升 live verified 仍需在本仓库内按上述清单重跑一遍并留存脱敏 receipt** —— 本次采集是在 workbuddy-cliproxy 侧做的探针式验证，不是本仓库的验收流水线产物。

国内 site 至今**没有任何本仓库采集**，只能标 `live_pending`。

`cli2api` 新增的 `deepseek-v4.1-flash` 精确 native ID、顶层 `reasoning_effort/reasoning_summary/verbosity` 与 300k/1M context-window 形状只作为 CB-N4 二级信号【E】。本仓库没有 CN 绑定账号 receipt 或冻结厂商目录，运行时 reviewed allowlist 仍不包含该 ID，状态保持 `live_pending` / `runtimeEnabled=false`；不得把它改写为 `deep-model`，也不得把这些模型专属字段泛化到其他 CodeBuddy 模型。

---

## 10. 订阅与计费接口（供应商节点"订阅信息"展示）

【D】2026-08-31 在国际站个人账号实测；路径与请求体来自 `workbuddy-switch`（面向 `www.codebuddy.cn` 的账号切换器）源码【C】，两站一致。

> 早前判断"CLI OAuth 凭据无法获取订阅信息"**已作废**——那次只做了 GET 扫描且未下探 `/meter/{action}` 层级。

### 10.1 端点

| 用途 | 方法 | 路径 |
|---|---|---|
| 资源摘要 | `POST` | `{billing-origin}/billing/meter/get-user-resource-summary` |
| 付费资源包 | `POST` | `{billing-origin}/billing/meter/get-user-resource-paid-packages` |
| 免费资源包 | `POST` | `{billing-origin}/billing/meter/get-user-resource-free-packages` |
| 旧订阅额度回退 | `POST` | `{billing-origin}/v2/billing/meter/get-user-resource` |
| 逐请求用量 | `POST` | `{billing-origin}/billing/meter/get-user-request-usage` |
| 签到状态（**不实现**） | `POST` | `{billing-origin}/v2/billing/meter/checkin-status` |

用量接口**没有 `/v2` 前缀**，加上会 404（`{"error_msg":"404 Route Not Found"}`）。鉴权仅需 `Authorization: Bearer {access_token}`，与数据面同一 token，无额外 scope。

`billing-origin` 不接受账号提供的任意 URL，而由 `site + domain` 在受控表内选择：Intl 的 `codebuddy.ai` / `workbuddy.ai` 分别落到对应 `www` origin；CN 的 `workbuddy.cn` 落到 `www.workbuddy.cn`，`codebuddy.cn` 与 `copilot.tencent.com` 落到 `www.codebuddy.cn`。所有请求携带 `X-Client-Platform: web`；CN 自然日使用 `Asia/Shanghai`，Intl 使用 UTC。

### 10.2 订阅信息投影

实现优先并行请求 summary / paid / free。任一路给出权威数组即可形成 modern partial result，detail 包按 package code 覆盖 summary 重复项；若三路都没有可识别资源结构，才回退旧 `/v2/...get-user-resource`。任一路 401 不得被兄弟请求的成功掩盖，而是交给统一控制面做至多一次 refresh + 全查询重放。

旧接口请求：

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
- 分页按 `total` 推进，按 `(requestId, requestTime)` 去重；空页但尚未达到权威 `total`、超过 100 页或响应超过 8 MiB 均视为不完整，不能误报成功。
- 观测到 `codewise-model-a9` 等**后端真实模型 id**，与 CLI 别名不同，可用于交叉验证 §6 目录。
- 失败请求也可能计费：`gpt-5.6-terra` 返回 `11133` 拒绝，用量表仍记 0.02 credit。重试策略须据此设上限。

### 10.4 实现约束

- 归入**控制面只读查询**，不进数据面热路径；结果随加密 accounts quota snapshot 缓存，成功缓存 30 分钟、失败缓存 5 分钟，手动强制刷新绕过缓存。
- 只保存最近 31 个自然日的聚合、按模型计数/credit 和最多 100 条安全明细；`input` / `inputTrunc` / prompt 永不复制。套餐成功但明细失败时保留套餐结果，并将 `providerUsage.status` 标为 `unavailable`。
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

可以直接复用的：`cli-external-link` 时序、`X-No-*` 头集合、cookie 亲和的 login client、10 分钟 flow TTL、5 分钟 refresh 提前量与强制流式。`sanitizeBlockedTemplates` 这类 prompt 内容改写不采纳；本实现只为满足上游硬约束，在缺失时前置最小 system message，不扫描或改写用户正文。

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

### 流式终止原因的类型边界（2026-10-10）

流式 decoder 与非流聚合器统一拒绝显式非字符串、非 `null` 的 `finish_reason`，防止数字、布尔值或对象绕开 finish 后业务帧栅栏并形成伪成功。`null` 与空白字符串沿用既有可丢弃兼容；合法非空字符串仍允许其后独立 usage 尾帧及唯一 `[DONE]`，成功只在 EOF 验证后提交。该局部收紧依据本仓库回归及 `cli2api` 已提交 WorkBuddy 聚合器的字符串解析，不宣称新的厂商协议或真实账号验收；Intl/CN receipt 与 CB-N4 仍待证。

### Choice 形状与 index 的双路径校验（2026-10-10）

流式 decoder 和非流聚合器共用单 choice 校验：一个 chunk 至多一个 `choices[]` 元素，该元素必须是对象，显式 `index` 必须可解析为 0。缺失 index、数值 0 和兼容字符串 `"0"` 保持可用；重复 index 0、非对象、非零或不可解析值在 `[DONE]` 前作为本地协议错误拒绝，不能被拼成一条回答或产生成功响应。空 choices 的独立 usage 尾帧与跨 chunk 的单 choice 续接保持。Intl/CN × Claude/Codex/Gemini × 流/非流的离线 HTTP 回环已覆盖；该规则不改变出站 WorkBuddy 请求，也不代表真实两站订阅验收。两份真实 receipt 与 CB-N4 仍待证。

### 工具调用显式 index 的校验（2026-10-10）

`tool_calls[].index` 缺失时继续用工具 ID 或分片位置归并，以兼容实测首帧可能没有 index；显式存在时必须可解析为 0～127 的非负整数，数字字符串如 `"0"` 仍兼容。`null`、非法字符串、负数或越界值不能降级为“缺省”，流式 decoder 在转发前和非流聚合器在合并前共用失败关闭校验。只读 `cli2api` WorkBuddy 聚合器按 index 汇集工具分片，提供差分信号而非本 Server 的严格成功规则；离线回环不证明两站会发送畸形分片。出站 wire、唯一 `[DONE]`+EOF 和两份真实 receipt 状态不变。

### 拒答分片的非流保真（2026-10-13）

CodeBuddy 流式清洗保留非空 `delta.refusal`；非流聚合现将多段拒答按原顺序拼接为 `choices[0].message.refusal`，并计入请求生命周期内存预算。只含拒答的回答不会因非流聚合而丢失可见内容；`finish_reason` 沿用上游值。固定 decoder→aggregator 反例在旧实现中得到空拒答，修复后保留完整文本。推荐 `cli2api` 的已提交清洗器也保留非空拒答，但其非流聚合未收集此字段；本项是目标 Server 的流/非流语义对齐，不推断两站厂商实际发送形状。出站请求、账号与站点绑定、唯一 `[DONE]`+EOF、receipt schema 和 Intl/CN 真实验收状态不变。


## R4：目录与并发发现的本地容量（2026-10-27）

目录使用128项B-tree，8分钟fresh、24小时最大stale与原身份键不变，访问/发布时惰性清理。缓存snapshot共享不可变payload及一份父预算owner，最后引用释放才退账；替换准入失败移除旧目录，当前新结果正常返回且不缓存，不能复活旧模型授权。single-flight最多128个注册，等待者/guard共享owner；新注册预算或数量不足时，在目录fetch前返回`503 / cc_switch_request_memory_exhausted`，不刷新、不重放、不罚账号。进程默认只观测，硬上限需按部署测量启用。逻辑稀疏索引估算不是RSS，控制面fetch/parse/不缓存结果scratch未声称全部归属。

R4-CB-01比较完整客户端tuple后继续使用本仓受审2.142.0及原目录身份。参考新CLI/IDE/Product/Session组合与Global desktop目录UA没有本仓Intl/CN两站真实回执，不能只换版本或把目录模型当成生成接受性。个人CLI的private-data=false保持，企业不扩张；WAF对system/developer/assistant及user/tool/reasoning字串的重写均不采纳。两个站点与CB-N4继续live_pending；现有响应头超时、非认证refresh分类、11148历史修复、唯一DONE+EOF和取消usage保留。没有新增统一milestone或socket-write计量，验证范围见计划§20.8。


CORE-03统一观察见增强计划§20.10.7：两站独立send/headers/read和原同账号恢复分attempt记录；sanitize后的文本、推理及实际工具字段才算首内容，空字段/role/usage/finish-only排除。原DONE+EOF、错误锁存、license/limit与恢复合同保持，HTTP仅body handoff，取消和真实末owner释放独立。计时不改变提示词、客户端身份、目录、usage或健康；两站真实验收继续live_pending。
