# Trae SOLO / raw_chat 单账号反代

> **状态：本地实现，`fixture_verified / live_pending`。** 本文是 CN SOLO、CN raw_chat、Intl raw_chat 三条单账号通道的权威合同，覆盖固定端点 OAuth/refresh、四协议入口、流式、工具、推理、图片边界、续接与资源预算。第 1–6 节描述 SOLO 基线，第 7 节补充本次三通道增强。仓库尚无真实 Trae 脱敏 receipt，不能据本地 fixture 声称 `live_verified`。不实现 IDE MITM、插件注入、桌面流量劫持、账号池或跨账号回退。

一个 Trae Provider Bundle 固定绑定一个同 ProviderType 的 Account：`trae_solo`、`trae_raw_chat` 或 `trae_raw_chat_intl`。初始请求、refresh、首次 eligible 401 恢复、目录与额度查询始终停留在该账号及其 auth/token generation；任何错误都不能改选账号、Provider 或 host。

## 1. 身份与固定端点

新增独立 `ProviderType::TraeSolo`（wire id `trae_solo`），不复用 Qoder 或 CodeBuddy 的凭据类型。SOLO 固定中国站点，四个目的地逐字节固定：

| Plane | Origin | Path |
| --- | --- | --- |
| OAuth | `https://api.trae.com.cn` | `/cloudide/api/v3/trae/oauth/ExchangeToken`、`/cloudide/api/v3/trae/GetUserInfo` |
| PKCE code exchange | `https://api.trae.cn` | `/trae/api/v3/oauth/ExchangeToken` |
| Agent | `https://trae-api-cn.mchost.guru` | `/api/agent/v3/llm_utils_chat`、`/api/ide/v1/get_detail_param` |
| Billing | `https://api.trae.cn` | `/trae/api/v2/pay/ide_user_ent_usage` |
| Browser | `https://www.trae.cn` | `/authorization` |

Credential/import payload 中的 `api_host`、URL 或 Origin 不参与目的地选择；若出现非空自定义 host，导入必须拒绝或丢弃后明确提示，不能静默发送 bearer/refresh token。

账号 profile 保存稳定 UID、显示信息与一组随机生成后持久化的 machine/device identity。账号 id 从 ProviderType、UID 和固定 CN site 域分隔派生。UID、site 或 device ownership 变化推进 identity generation，旧 Provider 绑定冲突而不是自动重绑。

SOLO 数据面身份常量为：legacy refresh client id `en1oxy7wnw8j9n`、app id `6eefa01c-1036-4c7e-9ca5-d891f63bfcd8`、IDE `0.1.52` / version code `20260811`、plugin `2.3.62834`、function `solo_work_lite`。新浏览器登录使用 IDE PKCE client `ono9krqynydwx5`，与推理通道分开。默认模型只用于新建表单建议，不得替代 live model detail entitlement。

## 2. OAuth 与 refresh

浏览器授权仅发送 `http://127.0.0.1:<Server 端口>/authorize`，不得使用 localhost、其他路径或预置 query。三通道都使用 `auth_from=trae`、IDE client `ono9krqynydwx5` 和 PKCE S256；CN 浏览器／授权码兑换使用 IDE `3.4.3`，Intl 使用 `3.6.2`，不改变推理通道的版本头。256-bit 随机 `login_trace_id` 是 10 分钟 TTL、一次性的 OAuth state，并由官网以 `loginTraceID` 原样回传。Server 精确匹配 state、回调地址、发起管理员 principal；手动提交还必须匹配当前 `flowId` 与供应商类型。未知、过期、取消、重复或并发消费均拒绝，不遍历或猜测其他 pending flow。

回调读取 `authCodeInfo.AuthCode`，以仅留在服务端内存中的 verifier 调用固定授权码兑换端点；CN 为 `https://api.trae.cn/trae/api/v3/oauth/ExchangeToken`，Intl 为 `https://grow-normal.trae.ai/trae/api/v3/oauth/ExchangeToken`。请求带稳定 machine/device 与 P-256 SPKI PEM 公钥。随后在同区域固定 OAuth origin 调用 `GetUserInfo`，核对 UID、enterprise 和重新登录的目标账号。access token、refresh token 与 UID 缺一不落库；`oauthClientId` 与加密设备私钥保存到同一 Account。callback 的 host/userRegion 不选择出站目的地，授权码不做 host fallback 或自动重兑；新 PKCE flow 拒绝降级为 refresh-token 回调。

本地／SSH 本地转发可自动 GET `/authorize`，只接受对应 loopback Host。该请求仅暂存完成结果，账号仍由原管理员的 status/complete 请求持久化。远程浏览器在官网确认授权后若无法连接本机 `127.0.0.1`，将地址栏中的完整最终回调 URL 粘回当前控制台即可，不要求 SSH 转发。`POST /api/accounts/trae/login/complete` 需要 `flowId`、`callbackUrl`，可附 `providerType`；Web invoke 始终提交三者。不要粘贴最初的官网授权链接。自动回调页禁止缓存、禁止 referrer，并清除地址栏中的授权码；请求 trace 只记录路径，不记录 query。

Web 手动提交会暂停后续轮询并使旧轮询的 UI 更新失效，同时等待正在执行的轮询：若它已经完成自动回调的账号保存，直接使用该结果；否则再提交手动回调。提交失败恢复轮询，原有到期时间不延长；取消后等待中的提交不再发出网络请求。

Refresh 复用现有 managed-account singleflight、durable receipt、CAS 与 generation fencing。旋转 refresh token 必须先形成 receipt，再原子写回同一账号。只有明确的 invalid/expired grant 将账号标为 `needs_relogin`；network、408、429 与 5xx 是瞬时错误，不改变登录状态。新 token 对应 UID 改变时 fail closed。

旧账号没有 `oauthClientId` 时保留原通道 client，特别是 SOLO 的 `en1oxy7wnw8j9n`；新账号按记录的 IDE client 续期。只允许该通道原 client 或 IDE client，不接受任意 client ID。已有设备密钥的同设备重新登录复用原密钥；设备私钥从不进入公开账号视图、授权链接或 Debug 输出。CN SOLO/raw 保留既有无 proof 的 cloudide refresh 合同；仅 Intl 使用已审的 DeviceProof，不从 v3 公钥注册推断 CN refresh 支持同样字段。

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

SOLO 基线包含一个 family/driver/空 option schema/conformance，以及 Claude、Codex、Gemini 三个 managed-account profile；raw_chat 的独立注册项见第 7 节：

- family `family.trae_solo`，driver `special.trae_solo`；
- profiles `claude.trae_solo`、`codex.trae_solo`、`gemini.trae_solo`；
- endpoint policy 固定，model policy 支持 single/passthrough；
- stream/tools 为 true，images 为 false，maturity 为 Experimental；
- conformance 在离线合同完成前为 `live_pending`，之后最多升为 `fixture_verified`，没有真实 receipt 不得声明 live success。

实现分层：账号身份放在 `domain`，OAuth/refresh 放在 `clients/oauth`，缓存与 scope 放在独立 runtime 模块，三 Surface 转换和 strict terminal 放在 `proxy`；所有 Account/flow/receipt 写入由 `state.rs` 域方法封装。

## 6. 恢复与验收门禁

唯一恢复路径是：首个下游提交前的 eligible auth failure → 强刷同一个 Account → 重新校验 Provider revision、Account identity/token generation、site/device scope → 完整重放一次。第二次认证失败、提交后错误、quota/rate limit、网络错误与任何 scope drift 都终止。

离线验收覆盖：官网 callback 格式白名单、PKCE state 精确绑定/TTL/一次性/并发、手动与自动回调、固定 host 与恶意 `api_host`；refresh rotating token/receipt/CAS/身份冲突；三 Surface payload；tool/reasoning；严格 `done`；EOF/error/重复 terminal；同账号单次 401；全部 generation drift；目录空/stale/fail-closed；额度脱敏。

真实验收需一份 CN Solo 脱敏 receipt，覆盖登录、refresh 轮换、model detail、quota、三 Surface 流/非流、tools、reasoning、首次与二次 401、五类业务码、截断流，以及日志、API 和持久化文件不泄露凭据。


## 7. 2026-11-06 单账号增强合同

外部协议证据冻结为 trae2api-hub `5ed619f`（1.4.3），只用于一次性源码审计。构建、测试和运行时均不读取该仓库。下列能力经 Server 独立实现，真实上游仍待验收。

| 通道 | ProviderType / Driver 后缀 | 鉴权与目录 | 工具 / 图片 |
| --- | --- | --- | --- |
| CN SOLO | `trae_solo` | 原冻结协议身份；`get_detail_param` 实时目录 | 原生结构化工具；图片明确拒绝 |
| CN raw_chat | `trae_raw_chat` | `Cloud-IDE-JWT`，UA `Trae CN/3.4.3`，版本码 `20260212`；独立 `model_list` | 可选提示词模拟；保留 image_url |
| Intl raw_chat | `trae_raw_chat_intl` | Agent `https://core-normal.traeapi.us`；Bearer、UA `TraeClient/TTNet`、版本 `3.6.2` / `20260924`、country `us` | 可选提示词模拟；保留 image_url |

raw_chat 独立 family / driver / 三 App profile，使用 `/api/ide/v1/llm_raw_chat`，模型写 `model_name`，生成参数写 `parameters`。CN / Intl 使用不同 ProviderType 与账号 ID 域隔离，既不能替换账号站点，也不能自动从 SOLO 回退 raw_chat。两条 raw 通道采用 passthrough 模型策略，客户端必须提交精确上游模型名；CN 仍必须在该账号的 raw 目录内，精确匹配且不接受 SOLO 的 auto/模糊别名。raw 健康测试也必须在请求或 Provider 中显式配置模型名，不继承 Claude/Codex/Gemini 的全局默认测试模型。Intl 暂无经过审查的可用实时目录接口，Discovery 明确 unsupported，不采用 CN 目录或静态表冒充可用权限。

raw OAuth client id 为 `ono9krqynydwx5`。CN OAuth 固定 `https://api.trae.com.cn`，Intl OAuth 固定 `https://grow-normal.trae.ai`，浏览器分别固定 `www.trae.cn` / `www.trae.ai`。旧 Intl 导入账号可通过 raw 的 `devicePrivateKeyPem` 选择性提供 P-256 SEC1/PKCS8 PEM 设备私钥；配置后必须正确签名，否则拒绝，不降级重试。签名为 ECDSA-SHA256 DER/base64，覆盖 METHOD、固定 PATH、client id、refresh token、毫秒整数 timestamp、随机 nonce；设备信息取同一账号的持久化 profile。设备证明 key 的变动推进身份代次；私钥按敏感字段加密落盘，公开 Account 视图同时移除敏感键并清除反射到其他文本中的原值。同 UID/enterprise/machine/device 的重新登录保留已有私钥，换设备不继承。轮换 receipt 仍先于 GetUserInfo 身份闭合持久化，单次恢复只限同账号认证失败。浏览器登录现按第 2 节的官网合同统一使用 IDE PKCE；参考 Hub 未提供对应浏览器登录闭环证据，真实浏览器授权接受性仍待验收。

raw 工具模拟默认关闭。显式 `driverOptions.traeToolSimulationEnabled=true`（或 Provider meta 同名字段）才会注入工具提示，并解析跨分片 `<|FunctionCallBegin|>…<|FunctionCallEnd|>`。Registry 的 native tools 保持 false。完整工具历史包含所有 system/developer、call ID、参数和工具结果，不自动裁剪。缺尾标记、无效 JSON、非对象参数、未知/错误工具名、必需字段缺失、required 无调用、禁止并行却返回多个调用、异常 finish 都失败；这些检查不是完整 JSON Schema validator，也不保证模型会遵从工具提示。

SOLO 和 raw 共用以下语义：

- 支持 LF/CRLF/CR、多行 data、传输控制字段、碎片化 UTF-8；SOLO 接受受限的 `Processing_*` 排队通知和空工具身份占位，已确认的非空身份禁止变化。
- 历史工具结果按精确 call ID 回填 name；孤立结果、重复 ID、缺失回执或冲突名称拒绝。空结果保持为空。缺参数仅在明确空 properties 且无额外约束的零参 Schema 下补 `{}`。
- 工具调用成功终态统一为 `tool_calls`，严格等待唯一 done 后的 EOF 校验；拒绝 EOF 假成功和重复 finalize。raw 也接受显式 `data: [DONE]`，不把无终态 EOF 合成成功。
- `max_tokens` / `max_completion_tokens` / `max_output_tokens` 统一映射，冲突、非正数或超过目录上限拒绝；temperature、top_p、stop 有明确验证；n 只允许 1。四入口在通用转换前验证源控制，保留显式别名并删除桥接推导的默认上限/思考档位。不支持的 reasoning 档位、max 模式与 seed（包括 0）拒绝。精确 thinking token budget 没有已审上游语义，Anthropic `thinking.budget_tokens`、正数 Gemini `thinkingBudget` 与 `reasoning_budget_tokens` 明确拒绝，调用方应改用 effort/level。`none` / disabled / Gemini budget=0 只省略档位，不能解释为保证关闭模型推理。
- 结构化输出没有经过审查的上游证据；显式 `json_object` / `json_schema` 请求返回参数错误。仅支持普通 text；不静默丢弃结构化约束。
- Anthropic 工具选择必须为 auto/any/none/tool；Gemini 支持 AUTO/ANY/NONE，显式 allowedFunctionNames 必须为非空、唯一且已声明的精确名称。未知模式、缺名称、非法并行开关、冲突别名与不能保证的 VALIDATED 模式返回 400，不降为 auto。
- raw 图片保留 HTTP(S) URL 或合法 image MIME/base64 data URI，URL 不允许嵌入凭据，Server 不主动下载媒体。四入口的消息图片与工具回执显式图片块均在转换前验证，base64 使用固定大小缓冲验证；SOLO 图片、文件 ID、非图片媒体明确拒绝。任意工具 JSON/字符串保持不透明，不递归当作媒体解释。
- Responses 在通用转换前恢复 `previous_response_id`，缓存保留完整输入消息、成功正文、工具调用及可重放推理摘要，当前 instructions 只作用于本次请求；按调用者/Share、Provider revision/runtime、账号实例和身份代次隔离。使用现有 10 分钟 TTL、2000 项/64 MiB、单项 8 MiB/200 items 上限。Share 需开启 previous-response cache；未知、过期、错作用域、被禁用的引用返回明确冲突。只在严格终态且下游协议转换成功后写缓存，不缓存中断/失败；合法 token 刷新保持同一已确认对话身份。
- 响应头、错误响应体和排队等待共用首段语义输出总期限，Transport 心跳不延长该期限。提交后每 15 秒发协议兼容心跳；心跳不推进上游 idle deadline。取消下游会释放上游响应、内存预算和在途 guard。缓存、解析、聚合、工具参数及协议转换纳入同一个 RequestMemoryBudget，耗尽返回稳定容量错误，不刷新重试。
- usage 检查非负整数和 details 范围，归一 prompt/completion/total、cached tokens、reasoning tokens，保留 cache creation 口径。Gemini 将 inclusive output 拆为 candidates 与 thoughts，二者不重复计数；额度、风控、模型错误均不消耗认证刷新预算。

管理入口同时覆盖 SOLO、CN raw_chat、Intl raw_chat，账号不能跨类型绑定；Web 功能开关说明工具模拟的性质。离线验收矩阵包括三通道 × Chat/Responses/Messages/Gemini × 流/非流 × 连续两轮工具调用/回执，另验证源控制拒绝路径、工具回执图片、完整文本续接、账号删除重建隔离、国际目录不发网、慢响应头/错误体/组合排队期限、15 秒心跳与 idle、取消释放、失败不缓存、刷新设备签名与私钥加密脱敏。真实模型权限、图片理解、国际设备注册状态和真实 token 轮换仍须凭据与上游 receipt 验收，不能由 fixture 推导。

## 8. 实施与整体 review 验收（2026-11-06）

规划内的单账号增强已实施并完成整体 review。Review 修复了普通正文续接被工具专用缓存过滤、Gemini 路径与 stream query 解析、跨协议推理 usage 丢失、源参数/媒体在通用转换时被静默丢弃、错误响应体未受首段总期限约束、设备私钥未纳入加密/脱敏规则等问题；同时补齐新增 Registry 类型对应的契约测试清单。最终 Trae 发送上下文使用结构体显式传递，Clippy 无警告。

本地验证结果如下，数量为整个 Server 仓库套件的计数，不作为外部项目或真实上游的质量评分：

- `RUST_MIN_STACK=67108864 cargo test --offline`：库测试 **4195 通过、0 失败、3 忽略**；API 集成测试 **138 通过**，Cursor fixture 与 lease payload 集成测试各 **1 通过**；binary/doc test target 均通过。另对最终发送上下文重构执行 Trae 专项，**44 通过**。
- `cargo check --offline`、`cargo clippy --offline --all-targets -- -D warnings`、`cargo build --offline` 通过；`cargo fmt --check` 和逐个列举全部 `src/**/*.rs`（含 `include!` 源文件）的 rustfmt 检查通过。
- 全部 Node 审计测试 **2135 通过**，smoke 脚本测试 **13 通过**，Phase 0 writer 证据检查及其 **6 项测试通过**；生成资产、覆盖矩阵、文档索引、依赖方向、状态写入纪律、出站 HTTP 规则与 diff whitespace 检查通过。
- Web 全套 **56 文件 / 391 测试通过**；locale 更新后的相关 **79 测试复验通过**，typecheck、build、i18n 与 dist 体积/引用审计通过。
- 本地服务 smoke 通过 health、version、Web、setup/login、API token 轮换及登录、Provider/Share 创建；使用独立临时配置，结束后清理测试进程和配置。
- `RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 scripts/release-readiness.sh` 已执行，**failures=0**，按脚本合同返回 `blocked`：其内部全套测试被显式跳过，且缺少真实 Router/Provider 输入与部署验收。本节单独记录的本地测试已通过；不将该离线检查描述为发布就绪。

没有执行真实 Trae 上游请求或部署；三通道真实接受性继续为 `live_pending`。

## 9. 官网 OAuth 失败修复与整体 review（2026-11-07）

直接根因是官网在查询登录状态前，对 `auth_callback_url` 执行 `^http://127\.0\.0\.1:(\d+)/authorize$` 白名单校验。原 Server 生成的 localhost、API 路径及预置 flow/capability query 均不符合要求，国内 SOLO 与 raw 入口因此在官网直接显示连接失败。官网仍有 legacy token 分支，缺少 PKCE 不能单独解释这个立即失败。官网脚本摘要、只读参考快照与采纳范围见 `PROTOCOL_EVIDENCE.md` 的 Trae 证据节。

本次按第 2 节修正三通道浏览器链接，并实现服务端 PKCE 授权码兑换、设备公钥注册、精确 state/回调地址/principal/ProviderType/TTL 绑定和远程手动粘贴。旧授权链接不可复用；需部署新版本后从控制台重新发起登录。现有账号保持原签发 client 兼容；国内续期不增加缺少证据的 Intl proof 字段。

整体 review 已修复以下问题，并加入负例回归：

- 同源 307 也会造成单次授权码被 HTTP client 重发；登录专用 client 继承统一出站配置并禁止重定向，无回退或自动重兑。
- 上游兑换错误可能反射 code/verifier；对外仅保留安全的固定诊断，不返回上游正文。
- state/scope 不能通过裁剪空白变成有效值，重复参数（含空值与别名）和 legacy token 混入一律拒绝；HTTP 默认 80 端口的浏览器规范化单独处理。
- 兑换期间取消或过期时，返回结果不得落库；自动回调也不能绕过原管理员持久化步骤。
- 手动回调与旧轮询竞争可能产生“已保存但界面失败”，或重复提交已被自动回调消费的 flow；Web 现在协调两条完成路径，并保留失败重试和取消屏障。新增 4 项 Web 回归覆盖旧轮询失败、手动失败恢复、自动完成与取消中的等待提交。

本轮最终本地验收基于 `5410aa69bce22ababf6cf4fe2093f836b2c5d3a5` 叠加本次 OAuth 修复，保留已有 Responses custom 工具提交；下述计数不沿用第 8 节的旧结果：

- `RUST_MIN_STACK=67108864 cargo test --offline`：库测试 **4211 通过、0 失败、3 忽略**；API 集成测试 **138 通过**，其他两个集成目标各 **1 通过**，合计 **4351 通过**。最终全量中包含 **56 项 Trae 测试**；binary/doc test target 均通过。
- `cargo clippy --offline --all-targets -- -D warnings`、`cargo check --offline`、`cargo build --offline`、`cargo fmt --check` 与包含全部独立 include 源文件的 rustfmt 检查通过。
- 最终源码上的 `node --test scripts/audit/*.test.mjs` **2135 通过**；smoke 脚本单测 **13 通过**，Phase 0 合同及其 **6 项单测通过**。Provider 覆盖、UI 矩阵、运行时合同、生成资产、文档索引、i18n、状态写入纪律、出站 HTTP 策略、依赖方向与 diff whitespace 检查通过。
- Web 全套 **56 文件 / 395 测试通过**，typecheck 与最终 build 通过；dist 体积与资源引用检查通过。组件测试包含上文的 4 项并发回归，不声称执行了真实浏览器授权。
- 最终二进制的 `scripts/smoke/smoke-local.sh` 通过 health、version、Web（200）、setup/login、API token 轮换与登录、Provider/Share 创建；使用动态 loopback 端口与独立临时配置，脚本完成后清理进程和配置。
- 离线 `RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 scripts/release-readiness.sh` 为 **failures=0、decision=blocked / blocked_inputs**；内部测试显式跳过，缺真实环境输入及部署验收，不代表发布就绪。上列本地套件已独立执行。

最终 `src/tests/assets/web-src/src/web-dist` 与 Cargo/build 输入共 839 文件的路径及内容 SHA-256 快照在验证期间保持一致，摘要为 `c81544cd2eeafc5d3e67b677e078871dd6f974c25bbd8e9d1c8340d5c039c6dc`。整体 review 未发现本次范围内尚未修复的本地阻断项。

真实账号的官网授权、设备注册与 token 续期仍为 `live_pending`；本地 mock/HTTP 回环和静态官网合同不等同线上闭环。上述验收完成时尚未提交、推送或部署。
