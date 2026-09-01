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
| Volcengine Coding Plan | China/Beijing | Anthropic Messages | OpenAI Chat | Volcengine plan API |
| Xiaomi MiMo Token Plan | China | Anthropic Messages | OpenAI Chat | unavailable |
| Xiaomi MiMo Token Plan | Singapore | Anthropic Messages | OpenAI Chat | unavailable |

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

## 真实验收

每个 region × Surface 至少需要一份独立脱敏 receipt，包含 Profile id、region、model、固定 origin/path 摘要、non-stream/stream、terminal、usage/quota state、HTTP/outcome 和 credential generation 是否稳定；不能保存 Key、raw body 或用户内容。

验收还必须覆盖错误 Key、429、5xx、坏/超大/截断 stream、Provider revision 与 credential rotation，并断言另一个 region、Surface、Provider 和任何 Account 的请求数为零。quota 为 `unavailable` 的 Profile 只验收诚实状态，不得临时抓 Cookie 或借用 PAYG/控制台余额。真实输入未齐备前，manifest 与报告必须继续标记 `fixture_verified` / `live_pending`。
