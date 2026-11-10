# WorkBuddy OAuth

> 状态：权威实现合同。本地 `fixture_verified` 与真实 `live_pending` 分开管理；没有真实账号回执时不宣称厂商接受性。

WorkBuddy 是腾讯办公 Agent 产品，与编程产品 CodeBuddy 分开注册。`workbuddy_oauth` 使用 `family.workbuddy_oauth`、`claude.workbuddy_oauth` / `codex.workbuddy_oauth` / `gemini.workbuddy_oauth` 和 `special.workbuddy_oauth`。固定绑定一个个人 Account，站点仅 `cn` / `intl`；不引入账号池、站点切换、跨 Provider/Account fallback 或桌面任务编排。每日签到是下述显式授权的中国站个人账号控制面扩展，独立于推理。

连接探针维持 1 token 输出上限。Codex 的预期输出上限终态仅在本次探针设置了上限、`response.incomplete` 的状态/原因为 incomplete / max_output_tokens 且 usage 证明达到该上限时视为健康；其他 incomplete、错误或真实断流仍失败。此检查不改生产推理的 partial-response 语义，详见 [CodeBuddy 测试合同](codebuddy-oauth.md#2026-11-02-自动模型测试与账号展示)。

| 策略 | CN | Intl |
| --- | --- | --- |
| 原生登录 platform | `workbuddy` | `workbuddy-ai` |
| OAuth / refresh origin | `https://www.codebuddy.cn` | `https://www.workbuddy.ai` |
| 推理与目录 origin | `https://copilot.tencent.com` | `https://www.workbuddy.ai` |
| generation / catalog 身份 | 受审 CLI 2.142.0 | 受审 CLI 2.142.0 |
| refresh source | `plugin` | `plugin` |
| 套餐 origin | 根据同站受审 token domain 闭合映射 | 根据同站受审 token domain 闭合映射 |
| 官方逐请求用量 origin | `https://www.workbuddy.cn` | `https://www.workbuddy.ai` |

产品、站点、认证平台、目录身份、推理身份和套餐来源独立建模。登录和旋转刷新后的 `/v3/config` 企业身份校验使用目录 origin，不能向国内 OAuth origin 误发目录请求。原生登录得到的凭据走 CLI 推理通道的联合接受性仍待真实验证。更高版本 / Desktop catalog 身份和新原生模型不能仅凭参考代码启用，旧 CodeBuddy CB-N4 门禁不变。

管理 REST：`POST /api/accounts/workbuddy/login/start|poll|cancel`；start 请求 `{ "site": "cn" }`，poll/cancel 请求 `{ "flowId": "..." }`。Web 通过现有 `auth_start_login` / `auth_poll` / `auth_cancel` 命令选择 `authProvider=workbuddy_oauth`，共用本仓 ManagedAccount UI。cookie jar、principal 绑定、TTL、poll lease、取消和容量上限沿用公共生命周期；另一产品的 route 不能读取、取消或完成该 flow。

供应商 Bundle 的可选 `buddyLoginSite` 保存新登录／导入的 `cn` / `intl` 偏好，与 CodeBuddy 共用管理合同。保存采用现有原子提交及版本校验，旧请求省略时保留原值。旧记录默认采用精确绑定账号的实际站点，再回退国际站；派生值不写草稿。UI 独立显示实际账号站点，登录／导入期间固定本次站点并互斥锁定控件。此偏好不修改已有 Account 的站点、身份、目录或转发 endpoint。

认证中心独立显示 WorkBuddy OAuth 与 CodeBuddy OAuth，沿用服务端 capability 门禁和已有账号接口。新登录／导入站点按产品保留在当前页面，折叠后保留、离页或刷新后重置，不修改供应商偏好或绑定；列表逐账号显示实际站点与安全显示名称，未知站点不推断。登录、导入及默认／删除／退出操作保持产品隔离和被引用账号保护；导入跨表单卸载成功时仍刷新共享缓存，但不能恢复旧表单状态。该接入只完善既有管理入口，不更改反代协议或账号身份。

Account ID 使用独立 `workbuddy-` 命名空间，稳定身份是 product × site × uid × enterprise。旧 CodeBuddy ID 与绑定保持；domain 同站品牌轮换不触发跨产品合并。相同 refresh token 在两个产品之间只允许一个 owner，避免旋转刷新竞态。导入要求显式 providerType、accessToken、refreshToken、profile.site/domain/uid；`productPlatform` 必须与产品和站点匹配。Server 不能解密桌面 `$wbEncrypted` 导出，应重新原生登录。extraHeaders 和任意 endpoint 覆盖不参与身份。

Claude Messages、OpenAI Chat / Responses、Gemini stream/nonstream 使用共同 canonical bridge，文本、工具、推理和图片输入能力由精确实时目录决定。公开 `/v1/models` 和管理发现复用相同 runtime、空目录语义、128 项 singleflight、8 分钟 fresh / 24 小时 transient-only stale；产品 Profile / Driver 进入 fingerprint，Account providerType 进入身份 digest。认证/协议错误 fail closed。只允许一次业务输出提交前的同账号 401 恢复；错误、取消、DONE+EOF、实际 usage 与内存边界见 [CodeBuddy 合同](codebuddy-oauth.md)。图片输入须由实际模型的 `supportsImages=true` 授权；图片生成、企业和未受审模型保持关闭。

`auto` 使用绑定账号的有效 CLI 目录：站点默认 ID 只有在目录启用且能力相容时优先，否则按 CLI 上游顺序选择满足请求能力的 reviewed 模型；显式 ID 不替换，空目录生成前拒绝。单模型健康检查遵循配置的上游模型，避免全局 GPT 模型/effort 覆盖；透传模式保留显式测试覆盖，未配置则为 `auto`。成功返回的检查结果及健康记录显示实际解析的模型，内部响应元数据不增加上游或下游 wire header。

账号公开视图新增可选 `displayName`：真实邮箱 → 脱敏手机号 → 昵称/名称 → 本地 ID；`email` 保持邮箱语义。已保存昵称的旧账号无需重新登录。登录/显式导入承载可选手机资料，正常旋转刷新在既有 `/accounts` 身份读取中回填；UID/enterprise 校验成功后才合并非空显示字段，缺失或不可显示字段保留旧资料。显示资料不改变账号 ID、产品/站点身份、认证代次和 Provider/Share 绑定；公开接口不返回原始手机号或完整 profile。

官方 credit 用量与本地 token 计量分开。按站点时区扫描近 31 个自然日，pageNum 固定 1，满 3000 条用最大时间推进 inclusive startTime，以归一时间与 requestId 去重；同秒饱和、窗口停滞、坏行和安全上限均不报 complete。套餐成功而官方明细失败时保留套餐并显示 unavailable。原始 prompt、账号私密字段和凭据不能进入日志或回执。

Server / Router 同时展示资源包额度与官方请求汇总，原始数据保留上游官方包名（可含 CodeBuddy），
各包使用已用/总额。Server Web 摘要按 [CodeBuddy 展示合同](codebuddy-oauth.md#2026-11-03-资源包与官方请求用量展示)
简化已知包名、汇总签到 credits，并仅显示仍有余额批次的最近失效倒计时。周期重置不替代积分失效；
未知期限、过期待刷新和部分数据分别提示。悬浮明细保留原名与各批次金额、期限。今日/本月官方 credits
接在同一段摘要中，自然折行；卡片省略
重复百分比、完整期限、采集时间、请求范围和说明文字；缺少金额时仍可显示已知使用率。
完整数据与原有校验保留。两者都是账号账本，
不与 Share 本地 Token/USD 配额混用，也不强行对齐不同采集时间的金额。
Router 新字段须显式协商，旧 reader 省略；具体字段、刷新和回填规则见
[Buddy 展示合同](codebuddy-oauth.md#2026-11-03-资源包与官方请求用量展示)。

参考固定为 `cli2api@fb2eb35`、`workbuddy-switch@22b318c`、`workbuddy-cliproxy@6528fdc`、`codebuddy2openai@8804ad6`，只作本次协议研究的只读证据。测试与发布不依赖外仓。真实验收需独立双站登录/旋转刷新、目录、四入口 stream/nonstream、工具、reasoning、429、取消和撤销回执；不能复用 CodeBuddy 的历史成功作为 WorkBuddy 成功。


`POST /api/accounts/buddy/import` 与 Web `auth_import_buddy` 共用一套本地校验，输入为 `{providerType, site, payload, preview}`。`preview=true` 仅验证并返回脱敏账号摘要；`preview=false` 在写入锁内先验证整个候选副本，再一次持久化。支持扁平 camelCase/snake_case 双 token、`{account, auth}` 和 `accounts`/数组批量格式，最多 32 个账号、1 MiB。产品、站点、平台或同一旋转 token 的 owner 冲突时整批拒绝；既不联系厂商，也不从输入复制任意 endpoint/header/raw。预览不替代真实身份与权益验收。Web 切产品/站点即清除导入内容和预览，导入处理中锁定站点；成功后刷新账号、套餐与额度视图。

本轮整体 review 已修复 WorkBuddy→Claude 的提前终止、后台额度刷新遗漏、额度卡手动刷新使用旧产品、公开目录遗漏出站检查、现代套餐错误分类丢失、官方明细缺失请求 ID 导致少计等问题。回归包含产品 × 站点 × 四入口 × 流/非流、签名 Share 目录、原生登录 platform/cookie、刷新与第二次 401、取消与截断、导入预览/原子回滚/持久化、同 UID 隔离和历史重复 token 刷新前拒绝。

独立真实回执合同为 `assets/contract/workbuddy-protocol.json`。执行 `node scripts/smoke/workbuddy-real-receipt.mjs --site cn` 或 `--site intl`，变量见 `.env.example` 中 `WORKBUDDY_*` / `CC_SWITCH_WORKBUDDY_*`。回执 family、账号 scope 字段、catalog/runtime/绑定摘要的 domain 均与 CodeBuddy 分开；另一产品回执不能互认。缺输入返回 `blocked_inputs / live_pending`，fixture 模式只允许 `contract_verified / live_pending`；真实通过需要仓库外 mode 0600 的私有回执与当前 Server 精确绑定再次核对。

2026-11-02 前一轮 WorkBuddy 接入验证：Rust 全量 4137 项通过、3 项既有忽略；Node 审计 2111 项、smoke 单测 13 项、Web 单测 289 项全部通过。格式、Clippy、类型检查、前端构建、契约及文档审计、本地启动 smoke 通过。离线 readiness 的 failures=0，但缺真实输入时 decision=blocked；双站真实验收仍为 live_pending / blocked_inputs，未执行部署，不据此宣称发布就绪。


2026-11-02 magpie 对比后的既有能力增强：产品独立的 CLI 身份策略进入缓存指纹；目录按实际 `cli` 通道筛选；补齐输出上限别名、保持约束的工具 Schema、11140/11153 分类、未知业务码的 HTTP 状态保留及明确带时区的重置时间解析。套餐 paid/free 采用有界完整分页；缺页明细不能覆盖汇总，积分失效与周期重置分开。账单无时区时间按 UTC+8 解析，查询窗口策略保持既有站点设置。请求准备减少重复 JSON 解析与完整克隆，保留内存、取消、终态和恢复边界。详细上限与语义见 [CodeBuddy 合同增量](codebuddy-oauth.md#2026-11-02-既有-buddy-能力补强)。

此次参考固定为 `magpie@834c6c7dfe6a6c1dc8c98fb2141105962908377f` 和 `opencode-workbuddy-auth@0.2.11` 源码提交 `486a68a54f7300e230023322bb1deb7d952fc310`。Desktop 5.9.3 / CLI 2.165.0 身份、CN 登录 origin 变化与两站查询窗口变更仍等待独立真实回执，不自动启用。新增项仅补强现有反代，不新增账号池、服务端会话、原生 Messages 上游、媒体、搜索或桌面能力。


本轮整体 review 覆盖产品/站点隔离、目录授权、参数与 Schema 的约束保持、账单完整性与脱敏、请求内存和终态恢复。已修正满页达到 TotalCount 仍需尾页探测、legacy 到期时间误报重置、跨明细资源重复计量与订阅周期关联，并使取消回归检查实际产品的账号租约。包 ID 冲突、总量漂移、多周期明细无法确定、缺 summary 等不确定状态保持失败关闭。

离线分配测量使用本轮开始前的源码快照与当前实际 payload 函数，合成 1 MiB 用户文本 + 8 轮各 256 KiB 的完整工具结果；每组 20 次、独立进程重复 3 组。额外申请堆内存峰值从 6,326,396 降至 3,165,166 字节，累计申请从 189,830,780 降至 63,422,740 字节，输出长度与摘要一致，轮次结束后无残留。该结果仅涵盖 payload 准备的分配请求，不含 allocator 开销、HTTP/TLS 或服务 RSS，不外推真实推理时延。


本轮最终验证：Rust 全量 4155 项通过、3 项既有忽略；Clippy（all targets，warnings as errors）、cargo check、cargo fmt 与包含 include! 源文件的 rustfmt 检查通过。`scripts/static-checks.sh` 全部通过，包含 Node 审计 2111 项、smoke 单测 13 项、Web 单测 289 项、前端类型检查、Provider/Writer/覆盖矩阵与文档/边界审计。全量验证同时修复了一处既有 USD 测试夹具：先启用 SQLite 再写关联 Provider，失败写入后比较完整旧授权列表，保留自动 owner 授权；未修改对应业务实现。离线 readiness 为 failures=0、decision=blocked，真实输入/部署检查仍未满足，不构成真实接受性或发布批准。

本轮 `scripts/smoke/smoke-local.sh` 启动验证通过；首次调用因初次编译超过脚本 30 秒启动等待而中止，预先完成可执行文件构建后重跑成功。所有最终源码与 review 时冻结的 Rust 摘要一致；验证阶段未部署，双站真实验收保持 `live_pending`。

## 2026-11-04 目录默认推理与 Claude CLI 兼容

两类 Buddy Driver revision 4 均采用 Registry 的 `buddy_upstream_default_v1`，覆盖 Claude、
Codex Chat/Responses、Gemini 及流/非流。忽略客户端请求级推理档位、预算和开关（含 none、
thinking.disabled 和合法冲突），在最终模型选定后显式发送精确目录默认档位。结构/类型
非法仍 400；默认按 defaultEffort → 旧 effort → 唯一档位解析，不猜菜单首项。明确非推理
模型省略参数，未知/多档无默认的固定模型在出站前 502，auto 按既有约束跳过该候选。
固定模型、账号/站点、工具/图片权限、历史思考、其他设置和严格终态保持。策略进入运行
指纹，既有 Provider 自动生效；API/选择器只读展示默认值，不增加配置开关。

本次 CN 对照中 kimi-k3-1 默认 medium、deepseek-v4-pro 默认 high；完全省略参数可调用，
但观察到的 reasoning tokens 与显式默认不同，不能保证同等推理能力。因此不使用纯省略
替代目录默认。该单账号观察不代表 CodeBuddy/国际站或新二进制的完整真实验收。

11128 独立处理：Claude→Buddy 只清理首条 system 开头的合法 `x-anthropic-billing-header:`
CLI 元数据行/块（须含 `cc_version` 与 `cc_entrypoint`），并规范逐字匹配、独占一行的默认
Claude Code 身份句。CN 同账号对照证实保留该 billing 元数据仍会被上游拒绝；其他指令、
正文、工具数据、非开头或不符合元数据格式的同名文本保持。不因 11128 刷新或重试。
完整合同见 [CodeBuddy §5.6](codebuddy-oauth.md)，
目录/最小对照证据见 `PROTOCOL_EVIDENCE.md`。

## 2026-11-04 国际站目录与套餐兼容

实时目录中 `onlyReasoning=true` 优先于同时出现的 `reasoning.canDisableThinking=true`：
有效的关闭推理能力固定为 false，继续使用目录声明的默认档位。此组合不再导致整份目录失败；
非法类型、明确非推理却携带推理配置、none 与强制推理冲突、默认不在允许档位内仍失败关闭。
既有受审模型范围、精确产品/账号/站点绑定、CLI 身份和终态合同保持。

现代套餐查询的 paid/free 码表补齐受审国际商品码，恢复原被漏掉的官方名称与周期信息。
真实 PackageName 优先；summary 缺少名称或名称等于编码时，006 / 035 分别回退到受审的
“CodeBuddy 个人体验版” / “版本基础用量”，未知码不猜名称。金额和期限只用上游证据；Server Web 现采用最近积分失效提示，
周期重置独立保留在明细中。归一后的数据沿既有配额快照同步给 Router，不新增 Router 侧码表。
真实只读对照确认同一 Intl 账号可取回 `Bonus Pack` / `Free Plan Subscription` 和各自
期限；这不等同于完整反代调用或远程部署验收，详见 `PROTOCOL_EVIDENCE.md`。


## 中国站个人账号每日签到

认证中心和 Provider 编辑页的“上游连接”均提供 WorkBuddy Account 的自动签到开关、`HH:mm` 时间和“立即签到”，默认关闭、09:00、`Asia/Shanghai`。签到设置按账号独立保存，无需保存 Provider；同账号绑定多个 Claude/Codex/Gemini Provider 或 Share 仍只有一个策略、任务和结果，两个入口共享状态。未选账号、绑定失效或状态读取失败时显示说明，不对替换账号操作；新登录站点偏好不改变绑定账号的签到资格。国际站、CodeBuddy、企业账号显示不支持，不向签到接口试探；Router 无新增签到操作。

固定 `https://www.codebuddy.cn`，POST `{}` 到 `/v2/billing/meter/checkin-activity-status`；仅 HTTP 404 回退 `checkin-status`。只有显式布尔“未签到”才提交 `daily-checkin`。缺字段、非法类型或矛盾状态不得视为未签到；HTTP 400 的明确“已签到”单独归一化。结果区分 success、already、inactive、authentication_required、retryable_error、error、unknown，不记录上游原文或凭据。上游活动不可用与 token 过期分别分类。

启动补跑当天到期任务，按稳定账号 ID 抖动 0–59 秒。签到前先持久化意图，账号操作锁、文件锁、身份 generation、进程内 incarnation 和站点/domain 保护并发、删除重建及过期回写；客户端断连不取消拥有者任务。超时、5xx 或不明确的提交响应记录 unknown，下次仍先查状态；尊重 Retry-After，按 5/15/60 分钟退避，每账号每天最多 6 次。自动关闭或时刻未到不启动新的自动签到；关闭后不撤销已发出的请求，结果仍记录，待确认意图仍可后台只读查询，明确未签到时不补发提交。跨北京时间午夜的响应不能确认新一天签到。

确认成功或已签到后，只刷新真实资源包，保留官方请求用量及其采集时间，不扫描历史请求；资源刷新失败单独标记待更新，最多 6 次、间隔 15 分钟。签到不凭空增加额度、不清除推理冷却、不重置分享额度，也不计入请求消耗。策略、待确认意图及最近结果随既有账号存储在 JSON / SQLite 中持久化，最近日志限制 30 天 / 100 条。

管理员 REST 为 `GET /api/accounts/:id/checkin?authIdentityGeneration=N`、同路径 `PUT` 策略（`authIdentityGeneration/revision/enabled/time`）及 `POST` 手动执行（`authIdentityGeneration`）。Web invoke `auth_get_checkin` / `auth_set_checkin_policy` / `auth_checkin` 使用相同编排与管理员鉴权，额外携带 `accountId`；GET 只读缓存，不触发网络。账号变化或旧 revision 写入返回冲突。

协议证据为 `workbuddy-switch@22b318c1f44b335c2222e444abcc2d0f74e160f7` 与 `cli2api@28ddb2fb8f0fa3c3563105130bab6c330ff551e1`，完整固定来源见本仓 `workBuddyCheckinExtension`。历史 CB-R2 拒绝记录保持；本次授权只扩展上述窄范围，不引入账号池或运营轮换。国际区参考策略有冲突，不能据此宣称支持。真实账号签到、奖励额度和期限验收均为 `live_pending`，本地 fixture 不替代真实回执。

## 积分获取能力边界

按用户明确决定，外部项目中获取积分的方式只借鉴上述“中国站个人账号每日签到”：
仅限 WorkBuddy 国内个人账号的 billing 自动签到，沿用既有资格、默认关闭和状态查证合同。
此决定不扩展为 CodeBuddy、国际站或企业账号的签到支持。

不接入新手任务、桌面端任务、增长活动连续登录、奖励领取/兑换、抽奖或国际每日活跃等
其他积分获取自动化，也不保留这些方向的预检工具、测试 fixture、后台任务或配置入口。
正常使用客户端后达标的奖励自动领取/兑换亦不在接入范围，不作为后续实施计划。

2026-11-06 正式环境研究中，三个国内账号的新手任务均未完成、兑换档位均锁定；
其中一个账号的真实网页任务及对话成功结束后，即时和后续回读均未计入新手完成、
今日活跃或连登。该样本不足以证明这些路径可可靠自动获取积分。
因此撤除此前新增的研究预检实现，仅保留研究结论作为不接入依据；
这不是断言上游永远无法实现，也不构成国内签到奖励真实到账的新增验收。
证据记录见根目录 PROTOCOL_EVIDENCE.md 的积分获取范围收敛条目。


## 2026-11-06 单账号反代兼容增量

生成协议收敛到 `src/proxy/buddy_wire.rs`，保持受审 CLI 2.142.0 的 endpoint、请求头、请求 ID 与紧凑 JSON 行为。现有身份 revision 已进入 runtime fingerprint；本轮没有升级客户端、切换 Desktop/VSCode、改变 refresh source 或启用新模型。

工具历史仅合并相邻的纯 assistant 调用声明：无文本、无推理或额外语义字段，调用参数完整、ID 唯一且合并后不超过 128 个；完整结果保持内容与顺序。完整批次中夹入 system/developer 指令时返回本地错误，避免移动指令或静默丢失工具结果；真正残缺历史仍沿用原合同。不从模型正文构造可执行工具调用。

实时目录保留有界排除原因：`upstream_disabled`、`not_enabled_for_generation_identity`、`not_reviewed_by_server`；未出现模型为 `not_authorized_in_bound_catalog`。这些原因用于生成拒绝，管理发现的已授权模型 raw 附三类排除数量；不公开被排除模型清单，不保留未受审 payload，不改变准入。全未受审目录仍按既有合同拒绝，空授权不回填静态模型。

推理档位选择与字段编码分离，继续使用实时目录默认档位及现有顶层 `reasoning_effort`。新版本、字段分支及模型仍需独立协议/真实回执，不凭外部实现自动启用。外部提交及暂不启用项见 `PROTOCOL_EVIDENCE.md`。

Responses 工具桥接支持 namespace 内混合 function/custom：custom 使用字符串 input 参数封装，历史、tool_choice 与流式/非流式回包保留类型、名称和 namespace。非法子类型和身份碰撞仍拒绝；此桥接不提供上游原生 grammar 约束保证。2026-11-06 修复的本地验证及真实验收边界见 `PROTOCOL_EVIDENCE.md` 的 namespace 内 custom 工具条目。


## R5 BD-01 产品、站点与平台证据（2026-11-10）

`assets/contract/r5-buddy.json` 冻结推荐 Hub/cli2api/workbuddy-switch 的十份 committed source 与十组本仓既有回归。CodeBuddy 双站 platform=CLI；WorkBuddy CN=workbuddy、Intl=workbuddy-ai。两产品推理继续使用受审 CLI 2.142.0，refresh source=plugin；参考 Hub CN=workbuddy 或可切换 Desktop/VSCode/CLI 不构成本仓必须改身份或借权的证据。真实原生登录与 CLI 推理联合接受性按产品×站点独立 live_pending。

本轮无新增生产 wire 缺口。复用精确目录默认 reasoning、namespaced tools/图片、多轮、单账号轮换 journal、计费 paid/free、官方用量连续扫描及取消/预算回归。保留严格 finish_reason、唯一 DONE、EOF 与截断工具拒绝；不采纳 Hub 默认 stop 聚合、WAF/系统提示清洗、跨身份重试、uTLS 变更或原生媒体新操作。实际验证见增强文档 §22.14。
