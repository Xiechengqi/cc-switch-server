# WorkBuddy OAuth

> 状态：权威实现合同。本地 `fixture_verified` 与真实 `live_pending` 分开管理；没有真实账号回执时不宣称厂商接受性。

WorkBuddy 是腾讯办公 Agent 产品，与编程产品 CodeBuddy 分开注册。`workbuddy_oauth` 使用 `family.workbuddy_oauth`、`claude.workbuddy_oauth` / `codex.workbuddy_oauth` / `gemini.workbuddy_oauth` 和 `special.workbuddy_oauth`。固定绑定一个个人 Account，站点仅 `cn` / `intl`；不引入账号池、站点切换、跨 Provider/Account fallback、签到或桌面任务编排。

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

Claude Messages、OpenAI Chat / Responses、Gemini stream/nonstream 使用共同 canonical bridge，文本、工具和推理能力由精确实时目录决定。公开 `/v1/models` 和管理发现复用相同 runtime、空目录语义、128 项 singleflight、8 分钟 fresh / 24 小时 transient-only stale；产品 Profile / Driver 进入 fingerprint，Account providerType 进入身份 digest。认证/协议错误 fail closed。只允许一次业务输出提交前的同账号 401 恢复；错误、取消、DONE+EOF、实际 usage 与内存边界见 [CodeBuddy 合同](codebuddy-oauth.md)。图像、企业和未受审模型保持关闭。

`auto` 使用绑定账号的有效 CLI 目录：站点默认 ID 只有在目录启用且能力相容时优先，否则按 CLI 上游顺序选择满足请求能力的 reviewed 模型；显式 ID 不替换，空目录生成前拒绝。单模型健康检查遵循配置的上游模型，避免全局 GPT 模型/effort 覆盖；透传模式保留显式测试覆盖，未配置则为 `auto`。成功返回的检查结果及健康记录显示实际解析的模型，内部响应元数据不增加上游或下游 wire header。

账号公开视图新增可选 `displayName`：真实邮箱 → 脱敏手机号 → 昵称/名称 → 本地 ID；`email` 保持邮箱语义。已保存昵称的旧账号无需重新登录。登录/显式导入承载可选手机资料，正常旋转刷新在既有 `/accounts` 身份读取中回填；UID/enterprise 校验成功后才合并非空显示字段，缺失或不可显示字段保留旧资料。显示资料不改变账号 ID、产品/站点身份、认证代次和 Provider/Share 绑定；公开接口不返回原始手机号或完整 profile。

官方 credit 用量与本地 token 计量分开。按站点时区扫描近 31 个自然日，pageNum 固定 1，满 3000 条用最大时间推进 inclusive startTime，以归一时间与 requestId 去重；同秒饱和、窗口停滞、坏行和安全上限均不报 complete。套餐成功而官方明细失败时保留套餐并显示 unavailable。原始 prompt、账号私密字段和凭据不能进入日志或回执。

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
