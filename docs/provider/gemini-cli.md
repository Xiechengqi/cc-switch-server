# Gemini CLI / Code Assist OAuth

状态：权威。适用 `claude.google_oauth` 与 `gemini.google_oauth`，使用 `oauth.gemini_code_assist` 和固定的 `gemini_cli` 账号。本页描述本仓可离线确认的合同；真实 Google OAuth/项目/订阅接受性保持 `live_pending`。

账号导入的 `raw.clientId` 或 `CC_SWITCH_SERVER_GEMINI_CLIENT_ID` 决定 OAuth client，本仓没有内置 Gemini client ID。授权/刷新 scope 为 cloud-platform、userinfo.email 和 userinfo.profile。Code Assist 请求使用本仓冻结的 GeminiCLI/0.31.0 身份，以及 `ideType=GEMINI_CLI`、`pluginType=GEMINI`；不借用 Antigravity 客户端、额外 scope 或 entitlement。

普通推理可以对同一账号调用 `loadCodeAssist` 发现**已经存在**的项目，并经现有 generation 检查、合并请求与持久化流程绑定项目。已有绑定项目可直接复用，quota 更新可以保留已导入项目。服务端返回的 current/paid tier 是订阅元数据；`allowedTiers` 中的候选层级、默认标记或 operation 提示都不授权选择层级、创建项目、轮询开通或接受条款。没有可用项目时请求明确失败，并遵循现有发现失败冷却；不会猜测 project ID。

首次开通保持 GM-01 研究门禁。已冻结参考中的 `onboardUser` 属于 Antigravity，其 GeminiExecutor 则使用 API key；两者均不能证明本仓可配置 Gemini OAuth client 的开通权限。未来启用必须有精确 client/scope、tier、条款/权限错误和 operation 轮询证据，并通过管理员显式动作、有界 poll/取消、身份 generation 核对与项目持久化验收。当前没有新增 onboarding API 或自动开通路径。

Gemini `countTokens` 继续走既有 AI Studio OAuth 合同，不依赖 Code Assist 项目发现。普通生成的原生 finish/error、流式终态和聚合校验继续适用。真实配额模型目录不会被静态 Gemini API-key 或 Antigravity 模型列表替代。

来源、身份范围、本地门禁和六组合成响应位于 `assets/contract/r5-gemini-cli.json`。本地回归检查无项目、候选免费/付费层级、operation 提示、已绑定项目和已有项目返回；它们不表示 Google 实际返回过这些合成响应，也不证明厂商接受性。
