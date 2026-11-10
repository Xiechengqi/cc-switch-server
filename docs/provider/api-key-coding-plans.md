# API Key Coding Plans 与 Ollama Cloud

本文定义 cc-switch-server 的 Provider-owned Coding Plan API Key 和 Ollama Cloud 边界。静态 Key 属于一个 Provider Bundle 的 credential generation，不创建可被推理选择的 Account；Server 不实现账号池、轮询、权重、quota/cooldown/concurrency 选号或跨 Provider、Account、region、Surface、credential rail fallback。

## 机器可读真值与漂移门禁

- 运行时真值仍是 `assets/contract/provider-registry.json` 中的 typed `codingPlan` Profile。
- `assets/contract/coding-plan-source-baseline.json` 固定本轮 OmniRoute 与 9router 的 commit 和逐文件 SHA-256，只收录用于 origin、route、catalog、quota 或 Ollama 合同的证据文件；它不读取桌面项目或 preset 仓库。
- `assets/contract/coding-plan-registry-manifest.json` 由 `scripts/audit/audit-coding-plan-registry.mjs` 生成。它逐 region × Surface 输出 fixed origin、protocol、credential slot/auth、routes、模型 capability、quota provenance、stream terminal、error policy、maturity、`fixture_verified` 与 `live_pending`。
- `node scripts/audit/audit-coding-plan-registry.mjs --check` 校验当前 Registry 与 manifest 等价；加 `--check-sources` 时还读取本地外部仓库，复核 commit 和每个 evidence file hash。外部目录缺失不影响普通离线门禁，但不能宣称完成了本轮 source drift review。
- 新条目只有在 fixed HTTPS origin、Provider-owned credential、精确 route、reviewed catalog、quota 的 supported/unavailable 语义、严格 stream terminal 与无 post-commit retry 全部可建模时，才能进入 typed Registry。通用 OpenAI-compatible upstream 不能自动升级成某个套餐。

当前 manifest 固定 10 个 Family、20 个 Profile、5 个区域标签，并要求每个 Family 恰有 Claude 与 Codex 两个 Surface：

| Family | 区域 | Claude | Codex | quota |
| --- | --- | --- | --- | --- |
| Kimi API Key | Global | Anthropic Messages | OpenAI Chat bridge | Kimi plan API |
| Zhipu GLM | China | Anthropic Messages | OpenAI Chat | Zhipu plan API |
| Zhipu GLM | Global | Anthropic Messages | OpenAI Chat | Zhipu plan API |
| Alibaba Coding Plan | China | Anthropic Messages | OpenAI Chat | unavailable |
| Alibaba Coding Plan | Global/Singapore | Anthropic Messages | OpenAI Chat | unavailable |
| MiniMax | China | Anthropic Messages | OpenAI Responses | MiniMax plan API |
| MiniMax | Global | Anthropic Messages | OpenAI Responses | MiniMax plan API |
| Volcengine Coding Plan | China/Beijing | Anthropic Messages | OpenAI Responses | Volcengine plan API |
| Xiaomi MiMo Token Plan | China | Anthropic Messages | OpenAI Responses | unavailable |
| Xiaomi MiMo Token Plan | Singapore | Anthropic Messages | OpenAI Responses | unavailable |

同名模型不能跨 Family、region 或 Surface 外推。manifest 只从每个 Profile 明示的模型读取 context window 与 text/image modality；没有独立字段时 tools 固定为 `not_inferred_without_explicit_model_evidence`。例如 GLM-5.3 只在有 OpenAI Coding rail evidence 的 Zhipu Codex Profiles 发布，不自动进入 Claude 或 Qoder rail。

## 凭据、quota 与错误边界

- inference 和 quota credential slot 都是 `/settingsConfig/...` 下的 Provider-owned secret；Provider API 对外只暴露 presence/generation，不创建推理 Account 行。
- fixed origin 和 path 在 RuntimePlan 编译及最终出站前各校验一次；URL 不能携带 userinfo、query、fragment 或跨 origin path。
- supported quota adapter 只能访问合同内固定 HTTPS endpoint，并逐项声明 credential role。quota 缓存包含 Provider credential source 与 generation；只允许相同 scope 的 transient failure 使用有界 stale。
- 没有稳定官方/plan quota endpoint 时，adapter 必须是 `unavailable`、endpoint 为空、credential slots 为空。Server 不读取 console Cookie、网页 session、HTML、余额 API 或另一个 plan 的 credential，也不以零值冒充已用尽/无限。
- Claude stream 必须以 `message_stop` 终止；OpenAI Chat 以 `[DONE]`，Responses 以 `response.completed`。terminal 前错误为 fatal，terminal 后数据失败关闭；typed Coding Plan 当前不在 401 或提交后重放，绝不通过另一个 Key、region、Surface 或 Provider 恢复。
- pricing/source/capturedAt 是协议与套餐证据时间，不是 live success。所有 20 个 Profile 当前仍为 `experimental`、`fixture_verified` / `live_pending`。

## Ollama Cloud

Ollama Cloud 不是 Account pool，也不是上述 20 个 `codingPlan` Profile。Claude/Codex 两个 Profile 共享一个 Provider Bundle 的 API Key generation；推理走固定 Ollama Cloud API，账号/用量只用于展示。

- Key 只属于 Provider，不创建 Ollama inference Account；不保存 Cookie、settings session 或 HTML。
- 只读投影并发执行 `POST https://ollama.com/api/me`（显式空 body）与 `GET https://ollama.com/api/usage`，使用敏感 Bearer header、15 秒 timeout、512 KiB 单响应上限和禁 redirect client。
- account 与 usage 是独立 section，允许 partial success。成功 section 可单独进入 cache；只有 rate-limit/transient failure 可读取相同 `(credentialSourceKey, credentialGeneration)` 的一小时内 stale，认证失败清空两段 cache，坏 JSON/超限/redirect 不准 stale。
- Claude/Codex Surface 共享同 generation single-flight。Provider 删除或换 Key 立即清除旧 scope；旧 Key 的在途结果提交前重新解析 Provider revision/generation，漂移后丢弃并按新 generation 重取。
- account/usage、0% utilization、session/weekly window 和 activity cost 都是 display-only，不阻断推理、不改变健康状态、不选择 credential，也不进入持久化 Account pool。

本地 fixture 已覆盖 exact method/path/header、optional schema drift、0..1 ratio、model/body bounds、redirect、错误脱敏、partial/stale、认证清理、Bundle single-flight、删除和 generation rotation。可选 `cargo test ollama_cloud_live_account_usage_from_env -- --ignored` 需要真实 `OLLAMA_API_KEY`；缺少 Key 或未执行推理、目录、usage 与错误验收时仍为 `live_pending`。

### R5 effort 兼容边界

Claude Messages、Codex Responses 与 Chat 的 Ollama Cloud 桥接统一处理显式 effort：`ultra` / `xhigh` → `max`、`minimal` → `low`、`none` / `off` / `disabled` → `none`；`low` / `medium` / `high` / `max` 保留。大小写与首尾空白可规范化，缺省或空字符串不注入 effort。未知值、类型错误及相互冲突的控制在发往上游前返回 400，显式关闭不能被当成缺省。

这些 alias 只表达请求转换，不证明 Cloud 的每个模型支持 max。官方本地 runner 的 Harmony 降档依赖模型配置与模板，本仓不凭 `gpt-oss` 字符串降低 effort，也不更换模型；上游拒绝时按既有错误合同返回，不降档重发。官方工具、length 终态与 token/cache usage 保留。三条 Cloud 入站均要求非空字符串 `finish_reason` 后的 `[DONE]`，缺失或非法终态、重复 `[DONE]` 和同批终态后数据均按失败结束；Claude 终态等到 `[DONE]` 才发布，保留其前的官方 usage。取消按既有状态码合同记录 client_cancelled 并释放上游 owner，不重放。冻结来源和合成差分见 `assets/contract/r5-ollama-cloud.json`，真实 Cloud 接受性仍 `live_pending`。

## 真实验收

每个 region × Surface 至少需要一份独立脱敏 receipt，包含 Profile id、region、model、固定 origin/path 摘要、non-stream/stream、terminal、usage/quota state、HTTP/outcome 和 credential generation 是否稳定；不能保存 Key、raw body 或用户内容。

验收还必须覆盖错误 Key、429、5xx、坏/超大/截断 stream、Provider revision 与 credential rotation，并断言另一个 region、Surface、Provider 和任何 Account 的请求数为零。quota 为 `unavailable` 的 Profile 只验收诚实状态，不得临时抓 Cookie 或借用 PAYG/控制台余额。真实输入未齐备前，manifest 与报告必须继续标记 `fixture_verified` / `live_pending`。


## R5 ST-01 静态 Profile 矩阵与 Bedrock 响应边界

`assets/contract/r5-static-profiles.json` 固定 26 个 Profile：Bedrock AK/SK 与 Bearer 各一、Nvidia/DeepSeek API 各两个 App，以及 20 个 codingPlan。审计将其与真实 Registry 的 app、origin/template、密钥槽、modelPolicy、driver、Bundle scope 和完整 codingPlan 合同交叉核对，并读取 Rust 的固定 origin 常量。旧 codingPlan 来源基线仍保留原提交，不冒称已对 R5 最新外仓重新全面审计。

两个实际 adapter 反例证明原 typed Bedrock 将 Converse JSON 原样交给 Claude，且未拒绝缺失终态/非法 usage。现将 Converse JSON 与 AWS EventStream 转为 Claude 形状，保留工具、thinking/signature、redacted thinking 与已观察 cache counters；请求中的 opaque thinking 历史及显式 thinking/top_k 保留，不替模型决定支持性。SigV4 的 date/region/service 与最终唯一 headers 保持既有合同；Bearer 与 AK/SK 独立配置，不互借、不 refresh。Claude DeepSeek API 修正为固定 `https://api.deepseek.com/anthropic/v1/messages`，Codex 保持 `/v1/chat/completions`；两者都不借 Web bearer。

合法 `model_context_window_exceeded` 原样保留；显式 stop sequences 请求对应额外响应字段，已观察 `stop_sequence` 原样返回，未观察的不猜。malformed model/tool 终态明确失败。

Bedrock streaming 校验 prelude/message CRC、帧/工具/状态顺序，必须取得 messageStop、合法 metadata usage 及完整 EOF 后才发布成功。异常不补成功，不跳损坏帧；未知内容或不完整工具参数明确失败。message_start 的零 usage 是流式占位，最终 metadata 才是已观察计数；缺失 cache 字段不编零。单帧 1 MiB、header 4 KiB、最多 128 block、单工具流参数 1 MiB，framing/header map/JSON 分配前保守预留工作集，解析/索引/输出受请求与父预算约束；帧上限不保证较小请求预算一定准入。取消释放实际 decoder/upstream owner，normalized Bytes 的预算随最后 clone/slice 释放。

本仓路径/转换/错误/资源验证与共享桥接测试不证明任何地区、模型、凭据 rail 已获厂商接受。26 个 Profile 各自真实权限、工具/媒体/usage/配额回执仍 live_pending；现有 MetadataOnly Account 不复制 Provider 私钥作为第二份推理权威。实际验证将在增强文档 §22.17 记录。
