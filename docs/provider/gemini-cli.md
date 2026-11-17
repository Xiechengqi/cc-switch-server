# Gemini CLI 退役兼容

状态：权威。`gemini_cli` 已退役；当前 CLI 方向为 [Antigravity / Agy](antigravity-oauth.md) 中的 `agy_oauth`。

旧账号、Provider 类型和隐藏的 `claude.google_oauth` / `gemini.google_oauth` Profile 保留只读识别，避免历史存储加载失败。旧记录可以查看和删除，但不能新建、导入登录、刷新 token/额度、发现模型或执行推理；后台也不会调度旧轨道。旧凭据不会自动转成 Agy 凭据。

需要继续使用时，应明确登录或导入一个 `agy_oauth` 账号，再创建并绑定新的 Agy Provider；核对可用模型和真实权限后自行处理旧记录。Server 不自动替换账号、改绑 Provider 或接受服务条款。

本次删除旧专属登录表单、模型目录客户端和出站 GeminiCLI 指纹；普通 `gemini` API Key，以及 Agy/Hub 共用的 Gemini 请求、流、Schema、`countTokens` 协议能力不受退役影响。源码删除可通过 Git 恢复。

`assets/contract/r5-gemini-cli.json` 及 PROTOCOL_EVIDENCE.md 的 GM-01 条目是历史冻结证据，不代表当前支持面，也不改写为 Agy 的真实验收结果。Agy 与 Hub 的真实接受性仍分别为 `live_pending`。
