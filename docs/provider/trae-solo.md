# Trae CN Solo 单账号反代

> **状态：本地实现完成，`fixture_verified / live_pending`。** 当前实现包含独立 `trae_solo` ProviderType、固定端点 OAuth/refresh、一次性 callback capability、三 Surface driver、严格 `done`、同账号恢复、目录、额度、管理 API 与 Web 入口。本文是 Server-native Trae CN Solo 上游 bridge 的权威合同；仓库尚无真实 CN Solo 脱敏 receipt，不能据本地 fixture 声称 `live_verified`。仍不实现 IDE MITM、插件注入、桌面流量劫持、账号池或跨账号回退。

一个 Trae Provider Bundle 固定绑定一个 `trae_solo` Account。初始请求、refresh、首次 eligible 401 恢复、目录与额度查询始终停留在该账号及其 auth/token generation；任何错误都不能改选账号、Provider 或 host。

## 1. 身份与固定端点

新增独立 `ProviderType::TraeSolo`（wire id `trae_solo`），不复用 Qoder 或 CodeBuddy 的凭据类型。当前只允许中国 Solo 站点，四个目的地逐字节固定：

| Plane | Origin | Path |
| --- | --- | --- |
| OAuth | `https://api.trae.com.cn` | `/cloudide/api/v3/trae/oauth/ExchangeToken`、`/cloudide/api/v3/trae/GetUserInfo` |
| Agent | `https://trae-api-cn.mchost.guru` | `/api/agent/v3/llm_utils_chat`、`/api/ide/v1/get_detail_param` |
| Billing | `https://api.trae.cn` | `/trae/api/v2/pay/ide_user_ent_usage` |
| Browser | `https://www.trae.cn` | `/authorization` |

Credential/import payload 中的 `api_host`、URL 或 Origin 不参与目的地选择；若出现非空自定义 host，导入必须拒绝或丢弃后明确提示，不能静默发送 bearer/refresh token。

账号 profile 保存稳定 UID、显示信息与一组随机生成后持久化的 machine/device identity。账号 id 从 ProviderType、UID 和固定 CN site 域分隔派生。UID、site 或 device ownership 变化推进 identity generation，旧 Provider 绑定冲突而不是自动重绑。

协议身份常量冻结为：client id `en1oxy7wnw8j9n`、app id `6eefa01c-1036-4c7e-9ca5-d891f63bfcd8`、IDE `0.1.52` / version code `20260811`、plugin `2.3.62834`、function `solo_work_lite`。默认模型只用于新建表单建议，不得替代 live model detail entitlement。

## 2. OAuth 与 refresh

浏览器授权 URL 必须携带至少 128 bit、一次性、10 分钟 TTL 的 callback capability。Server 只接受与 exact flow、预期回调字段和发起管理员会话绑定的 capability；未知、过期、已消费或并发消费均拒绝。禁止遍历 pending map 或消费“任意一个”登录。

回调取得 authorization code 后，仅向固定 OAuth origin 调用 `ExchangeToken`，再调用 `GetUserInfo` 确认稳定 UID。access token、refresh token 与 UID 三者缺一不落库；敏感字段只经加密 Account store 持久化。

Refresh 复用现有 managed-account singleflight、durable receipt、CAS 与 generation fencing。旋转 refresh token 必须先形成 receipt，再原子写回同一账号。只有明确的 invalid/expired grant 将账号标为 `needs_relogin`；network、408、429 与 5xx 是瞬时错误，不改变登录状态。新 token 对应 UID 改变时 fail closed。

## 3. 数据面合同

Agent 请求固定使用：

- `Authorization: Cloud-IDE-JWT {access_token}`、`X-Cloudide-Token`、`X-Ide-Token`；
- bound UID、machine/device id、`X-App-Id` 与冻结的 IDE/plugin/version 头；
- `function: "solo_work_lite"`、`stream: true`、请求模型写入 `config_name`；
- tool 的 `function.parameters` 编码为 JSON 字符串；assistant tool call 用 `function_call`；
- reasoning 映射为 `reasoning_effort_level`，仅显式 max 模式写 `is_max_mode: 1`。

Claude Messages、OpenAI Chat/Responses 与 Gemini 输入先进入本仓库 canonical chat，再由独立 Trae driver 生成请求；上游始终流式，下游非流式由严格聚合器产生。额外 header 不能覆盖 token、UID、device、app/version 或 host 身份。

响应状态机识别 `metadata`、`output`、`token_usage`、`done`、`error`。只有完整且唯一的 `done` 是成功 terminal；EOF、解码错误、`error`、重复 terminal 与 terminal 后业务数据都失败，绝不合成 `[DONE]`。流一旦向下游提交业务输出便不能 refresh/replay。

业务码至少结构化区分：`1001` 认证、`1005` plan quota、`4008` Solo credits exhausted、`4001` 模型/版本无效、`4011` hard rate limit。只有 HTTP 401 或经 fixture 证实等价的 pre-commit `1001` 可消费一次同账号 refresh/replay 预算；其他业务码不是账号切换信号。

## 4. 目录、额度与缓存

模型详情只从固定 Agent origin 的 `/api/ide/v1/get_detail_param` 获取，按 App、Provider revision/runtime、Account、auth/token generation、device identity 与 CN site 精确作用域 single-flight 缓存。成功空结果是权威；仅 network/408/429/5xx 可读取完全相同 scope 的有界 stale；401/403、坏 JSON、未知成功结构或 generation drift fail closed。

额度只读调用固定 Billing origin 的 `/trae/api/v2/pay/ide_user_ent_usage`。输出仅保留计划名称、Solo entitlement、credits/usage、周期与到期等非敏感投影；不回传 token、UID、机器标识或原始响应，也不实现签到、积分领取或其他运营操作。

## 5. Registry 与代码落点

Registry 新增一个 family/driver/空 option schema/conformance，以及 Claude、Codex、Gemini 三个 managed-account profile：

- family `family.trae_solo`，driver `special.trae_solo`；
- profiles `claude.trae_solo`、`codex.trae_solo`、`gemini.trae_solo`；
- endpoint policy 固定，model policy 支持 single/passthrough；
- stream/tools 为 true，images 为 false，maturity 为 Experimental；
- conformance 在离线合同完成前为 `live_pending`，之后最多升为 `fixture_verified`，没有真实 receipt 不得声明 live success。

实现分层：账号身份放在 `domain`，OAuth/refresh 放在 `clients/oauth`，缓存与 scope 放在独立 runtime 模块，三 Surface 转换和 strict terminal 放在 `proxy`；所有 Account/flow/receipt 写入由 `state.rs` 域方法封装。

## 6. 恢复与验收门禁

唯一恢复路径是：首个下游提交前的 eligible auth failure → 强刷同一个 Account → 重新校验 Provider revision、Account identity/token generation、site/device scope → 完整重放一次。第二次认证失败、提交后错误、quota/rate limit、网络错误与任何 scope drift 都终止。

离线验收覆盖：callback capability 精确绑定/TTL/一次性/并发；固定 host 与恶意 `api_host`；refresh rotating token/receipt/CAS/身份冲突；三 Surface payload；tool/reasoning；严格 `done`；EOF/error/重复 terminal；同账号单次 401；全部 generation drift；目录空/stale/fail-closed；额度脱敏。

真实验收需一份 CN Solo 脱敏 receipt，覆盖登录、refresh 轮换、model detail、quota、三 Surface 流/非流、tools、reasoning、首次与二次 401、五类业务码、截断流，以及日志、API 和持久化文件不泄露凭据。
