# Code Agent 回归矩阵

本矩阵对应 Z3/Z8。真实 provider/token 不存在时，只能执行本地 contract 和 mock 回归；真实账号回归不得伪标完成。

AD3 已将本页矩阵固化为 `docs/provider/regression-matrix.json`。`scripts/smoke/code-agent-regression.sh` 会先运行 `scripts/smoke/code-agent-matrix-summary.mjs`，输出：

- `matrixTotal`：矩阵组合总数。
- `matrixRunnable`：当前环境变量齐备、可运行的组合数。
- `matrixSkipped`：缺少真实输入而跳过的组合数。
- `matrixSkeleton`：仍包含 skeleton/mixed adapter 的组合数。
- `staticNativeFamilies`：本地静态 adapter contract 已覆盖的 provider family。
- `staticPlannedFamilies`：已有请求计划或签名契约，但仍未启用真实转发的 provider family。
- `staticRemainingFallbackFamilies`：仍是 skeleton/manual/import-only 的 provider family。
- `fixtureEvidenceComplete`：所有 case 的必需真实验收维度都有 `passed` 证据和脱敏 evidence 路径。

这些字段会写入 acceptance evidence；没有真实 token 时只能说明 contract pass + real skipped，不能说明真实 provider 已通过。

证据同时写入 `verificationState`：离线合同确实执行并通过后为 `contract_verified`；合同未执行时保持 `blocked_inputs`，不能借静态矩阵声明升级状态。只有 `RUN_REAL=1`、`RUN_CONTRACT_TESTS=1`、合同测试确实命中并通过、矩阵输入和实际探测均无跳过、`STREAM_PROBE=1`、`REQUIRE_STREAM_USAGE=1`，并且 `MATRIX_LIVE_EVIDENCE_FILE` 对每个 case 的全部必需维度都记录为 `passed` 时才是 `live_verified`。`verificationScope=configured_matrix_routes` 只覆盖本次实际配置的路由，不代表所有 Provider family 均已真实验证。

`blockerGroup` 记录最高优先级缺口，`checks.blockedGroups` 记录全部缺口。固定分类为：`contract-incomplete`（合同未运行、未通过或矩阵为空/计数不一致）、`missing-matrix-input`（矩阵要求的 token/share/URL 缺失）、`missing-stream-evidence`（stream 或 usage 硬门禁未启用）、`missing-live-fixture-evidence`（真实维度清单不完整）、`live-run-disabled`（`RUN_REAL!=1`）和 `live-probe-skipped`（矩阵输入齐备但仍有探测跳过）。真实探测失败使用 `live-probe-failed`，不会伪装成缺 token。合同或矩阵自身不完整时 evidence `status=blocked`；只有合同基线已通过、等待真实输入/证据时才使用 `ready-with-known-external-blockers`。

`MATRIX_LIVE_EVIDENCE_FILE` 必须是私有、脱敏的 JSON 文件，不提交真实响应或凭据。格式如下；`cases` 必须覆盖矩阵中的每个 case id，`checks` 必须覆盖 `requiredFixtureFields` 的全部字段：

```json
{
  "schemaVersion": 1,
  "cases": {
    "claude-local-messages": {
      "evidencePath": "/private/evidence/claude-local-messages.json",
      "checks": {
        "non_stream": "passed",
        "stream": "passed",
        "tool_function": "passed",
        "image_media": "passed",
        "reasoning_thinking": "passed",
        "cache_usage": "passed",
        "upstream_4xx": "passed",
        "upstream_5xx_timeout": "passed",
        "client_cancel": "passed",
        "final_usage": "passed",
        "request_log": "passed"
      }
    }
  }
}
```

## 入口维度

`staticCoverage` 只表示本地 contract 和 fixture 已覆盖；`adapterStatus: mixed` 仍会被计入 mixed/skeleton，直到真实 provider 和 Router Share/Gateway URL 都有 non-stream/stream 证据。

Kiro text 的 2026-08-30 回归合同覆盖单一模型 resolver、绑定账号目录授权与 token limits、profile-aware 有界 discovery、request-scoped session/cache、严格工具桥与 EventStream、独立 keepalive 时钟、同账号 throttle/cooldown，以及不进入 token quota 的 supplemental credits。目录 scope 进一步覆盖 Provider revision/runtime fingerprint、Account auth/token generation、profile 与 region；成功空目录权威，仅 transient failure 可读同 scope last-known-good，坏 JSON/缺 models/超 2 MiB/认证失败/代际漂移均失败关闭，首次 401 只强刷原账号一次。三个 text surface 保持 `fixture_verified`/`live_pending`，Gemini 不开放。Amazon Q 已分离为独立 typed family，Kiro 凭据、目录、quota、endpoint identity 与 request origin 都不能参与 Amazon Q；两者只共享已审计的纯转换与 EventStream decoder。全程不允许账号池、轮换、跨账号目录 union 或跨 Provider failover。

Amazon Q Developer 的 2026-08-31 回归合同以官方 `amazon-q-developer-cli` 提交 `15cc8f3cd18c` 为 wire 真值。`amazon_q_oauth` Account 独立完成 AWS SSO OIDC device register/start/poll/refresh，`special.amazon_q` 只使用 CLI root endpoint、target、UA 和 `origin: CLI`，支持 Claude Messages 与 Codex Responses/Chat。`ListAvailableModels` 处理分页、`defaultModel`、成功空目录和 exact Provider/runtime/Account/auth+token-generation stale；`GetUsageLimits` 使用同一 bearer/region。首次 eligible 401 只刷新原绑定账号并在下游提交前重放一次。Claude/Codex Profile 均有可创建桥，Kiro Account 在网络请求前失败关闭。本地状态为 `fixture_verified`/`live_pending`；真实 Builder ID/IdC 的登录、目录、额度、双 Surface non-stream/stream/tools/images、401/429、取消和撤销仍需脱敏 receipt。

Web Session 的 2026-08-31 回归合同将 Grok Web 与 Perplexity Web 各注册为 Claude/Codex 两个 hidden typed Profile，统一使用 Provider-owned Cookie rail；不创建或绑定 Account，不允许池、轮换、自动刷新、Bearer/API Key/extra-header 借道或跨 Provider fallback。Driver 固定 method/path/origin/referer/UA 与 Cookie allowlist，禁用 redirect/cookie jar/cross-origin，并丢弃 `Set-Cookie`/`Location`。Claude Messages/count_tokens 与 Codex Responses/Chat 只开放文本；tools/images 等在零网络前拒绝。Grok NDJSON 与 Perplexity SSE 在任意分块下要求严格唯一终态；401/403 使当前 credential generation 失效，只有显式重新导入才能恢复，取消也不重试。静态目录明确 `entitlement=not_asserted`。本地状态为 hidden/Experimental、`fixture_verified`/`live_pending`；Gemini 明确 unsupported。

Cursor 的 2026-08-30 回归合同在既有 OAuth CLI/API-key SDK 双 rail、ServerConfig trust、park/resume、MCP wrapper、图片与绝对 business-output deadline 上，增加 fresh exact-scope live catalog 模型语义：只有 API-key `/v1/models` 在同一 App、Provider revision/runtime、credential generation 与 key digest scope 中仍 fresh 时，完整 `*-fast` ID 才原样进入 protobuf；stale 或跨 scope 目录不参与推理授权。last-known-good 最多保留一小时，成功空目录权威；ServerConfig field 27、`agentUrl` 或 `agentnUrl` 重复均失败关闭。289 项聚焦测试通过，状态为 `fixture_verified`/`live_pending`，两条 rail 永不互相 fallback。

Gemini Code Assist 的 2026-08-30 合同将 `oauth.gemini_code_assist` discovery 接到 Provider 显式绑定账号的 `retrieveUserQuota` model buckets。目录只保留去重后的 Gemini entitlement；401 只刷新同账号一次。仅 408/429/可重试 5xx 可返回相同 `authIdentityGeneration` 的 stale 目录，401/403、代际漂移、持久化失败或无同 scope 缓存均失败关闭；不与静态或其他账号目录合并。状态为 `fixture_verified`/`live_pending`。

Antigravity/Agy 的 2026-08-30 合同将 `special.antigravity` 与 `special.agy` discovery 接到当前 Provider 显式绑定账号的 `POST /v1internal:fetchAvailableModels`。目录保留模型级 quota/reset、thinking/image/token/MIME capability、deprecated alias，以及 Gemini/Claude/GPT/other family；缓存键包含 rail、Account 和 `authIdentityGeneration`，两条 rail 互不借用。成功空目录权威；只有网络、408、429、5xx 可返回同身份旧目录，401/403、绑定/代际漂移与坏成功响应失败关闭。TokenRouter 未提供可独立复现的 weekly-summary endpoint，因此本轮不合成周额度。状态为 `fixture_verified`/`live_pending`。

Grok 的 2026-08-30 合同将 `/v1/models` 收敛为固定 Provider/runtime + Account + auth/token generation 的动态 entitlement 目录。成功空目录权威；只有 network/408/429/5xx 可使用同 scope、24 小时内的 stale，401/403、坏成功响应、无绑定、持久化降级或任何代际漂移均失败关闭，首次 401 仅强刷原账号一次。目录逐模型输出保守 family/capability manifest，不能由 model id 证明的媒体和搜索 entitlement 保持 unknown。异步视频 ownership 升至 schema v4，task key 覆盖 Share/用户/task kind/id/Provider/Account/auth generation/runtime/plane，v1-v3 精确迁移。181 项本地 Grok 测试通过，真实 xAI 仍为 `live_pending`。

GitHub Copilot 的 2026-08-30 验收合同要求 Claude、Codex、Gemini 三个显式 Provider ID 绑定同一个 `github_copilot` Account 和 `authIdentityGeneration`。`scripts/smoke/copilot-real.mjs` 分别经管理 API 获取 fresh entitlement 目录，复核 GitHub domain、受信 API origin、picker/policy、endpoint、limits 与 tools/vision/reasoning metadata，并只从三份目录交集选模型；随后验收原账号 premium quota，以及三 Surface 的非流、流、强制 tool、usage 和唯一终态。`scripts/audit/copilot-real.test.mjs` 已用本地 HTTP mock 覆盖 PASS、blocked-inputs SKIP 与 secret redaction；这只证明 harness 合同，github.com/GHES 均继续标记 `live_pending`。

DeepSeek Web 的 2026-08-30 合同只在 Claude Messages Surface 注册 `special.deepseek_account` Native Driver。导入为严格 bearer-only；生产 origin 固定 `chat.deepseek.com`；session/PoW/completion 状态绑定 Provider/runtime、Account auth/token generation、Share、用户、客户端 session 与 reviewed model。仅复用 session 的 400/404/409 可在提交前用原账号重建一次；401/403/429/5xx、代际漂移与提交后错误均终止。reviewed discovery 与 provider network test 已由 22 项 DeepSeek 聚焦测试覆盖，状态为 `fixture_verified` / `live_pending`；Codex、Gemini 与 images 明确 unsupported，不能落入 generic fallback 或 `deepseek_api`。

API Key Coding Plans 的 2026-08-30 manifest 从 Registry 生成 10 个 Family、20 个 region × Claude/Codex Surface contract；外部 source baseline 固定 OmniRoute、9router commit 与 20 个 evidence file hash。生成审计逐项门禁 fixed HTTPS origin、Provider-owned credential、route、catalog、quota provenance、terminal、retry 与 maturity，quota unavailable 不允许 endpoint/Cookie，tools 无显式证据不外推。Coding Plan 29 项通过。Ollama 另以 Provider Bundle key 并发读取官方 `/api/me`/`/api/usage`，已覆盖 partial、redirect/body limit、同 generation stale、认证清理和 rotation fence；24 项通过，真实 Key 测试保持 ignored/live pending。

| App 入口 | 路径 | 已有 native/static contract | 仍未真实关闭 |
| --- | --- | --- | --- |
| Claude | `/v1/messages` | Claude API/Auth/OAuth、Codex Responses、Gemini/Gemini CLI、OpenRouter、Antigravity/Agy、Ollama、Nvidia、DeepSeek API key；Kimi native Messages/count_tokens、权威空目录与错误分离、transient-only exact-generation stale、无静态 entitlement fallback、auth/token 代际 thinking replay 与同账号单次 401 已 fixture-verified；Qoder COSY 动态 site×route effort/context、签名时钟/UUID nonce、精确 account-generation model/session/quota scope、SSE terminal 与同账号单次 pre-commit replay 已 fixture-verified；Kimi/GLM/Alibaba/MiniMax/Volcengine/MiMo API-key coding-plan 固定 route/model/quota contract 已覆盖；Kiro Claude→CodeWhisperer、GitHub Copilot Claude→Chat 与 DeepSeek Web session/PoW/strict-terminal 均有 fixture-verified Native 合同；hidden Grok/Perplexity Web Session 的文本 Messages/count_tokens、固定 Cookie rail、严格终态/取消和静态非 entitlement 目录已 fixture-verified；Cursor AgentService text/image/tool、精确 session/index scope、初始/恢复绝对业务输出 deadline、builtin 协议拒绝、同绑定 credential/catalog/401/429 已 fixture-verified | Kimi、Qoder 与 API-key coding plans 真实登录/订阅/inference/tool/quota 仍 pending；Web Session 保持 Experimental/live-pending 且 tools/images 不开放；Cursor 仍为 Experimental/live-unverified；Bedrock 只有 SigV4/Converse request parts；DeepSeek Web 仍需真实 bearer 验收；Copilot github.com/GHES、Kiro 与 Cursor 真实账号验收仍 pending |
| Codex Responses | `/v1/responses` | Codex/OpenAI-compatible、OpenRouter、Ollama、Claude Messages、Gemini/Gemini CLI、Antigravity/Agy、Nvidia、DeepSeek API key；Kimi Chat bridge、权威空目录与错误分离、transient-only exact-generation stale、无静态 entitlement fallback、K3 effort、auth/token 代际 thinking replay 与同账号单次 401 已 fixture-verified；Qoder COSY Responses bridge、动态 site×route effort/context、签名时钟/UUID nonce、精确 account-generation model/session/quota scope、SSE terminal 与同账号单次 pre-commit replay 已 fixture-verified；Kimi/GLM/Alibaba/MiniMax/Volcengine/MiMo API-key coding-plan 固定 route/model/quota contract 已覆盖；Kiro 与 GitHub Copilot 已 fixture-verified Native；hidden Grok/Perplexity Web Session 的文本 Responses、固定 Cookie rail、严格终态/取消和静态非 entitlement 目录已 fixture-verified；Cursor Responses lifecycle、精确 continuation/tool scope、绝对 deadline、builtin 拒绝与同绑定 credential/catalog/401/429 已 fixture-verified | Kimi、Qoder 与 API-key coding plans 真实账号/订阅验收仍 pending；Web Session 保持 Experimental/live-pending 且 tools/images 不开放；Cursor 仍为 Experimental/live-unverified；Bedrock planned；DeepSeek Web 明确 unsupported；Copilot github.com/GHES 与 Kiro 真实账号验收仍 pending |
| Codex Chat | `/v1/chat/completions` | 与 Codex Responses 同一 provider family；Kimi 精确 Coding Chat endpoint、权威空目录与错误分离、transient-only exact-generation stale、无静态 entitlement fallback、K3 effort、auth/token 代际 thinking replay 与同账号单次 401 已 fixture-verified；Qoder COSY Chat bridge、动态 site×route effort/context、签名时钟/UUID nonce、精确 account-generation model/session/quota scope、SSE terminal 与同账号单次 pre-commit replay 已 fixture-verified；Kimi/GLM/Alibaba/MiniMax/Volcengine/MiMo API-key coding-plan 固定 route/model/quota contract 已覆盖；Kiro、GitHub Copilot 与 Cursor Chat text/tool/finish/usage 均有本地 fixture，Cursor 共用精确 scope、deadline、builtin 拒绝和同绑定 retry 合同；hidden Grok/Perplexity Web Session 的文本 Chat、固定 Cookie rail、严格终态/取消和静态非 entitlement 目录已 fixture-verified | Kimi、Qoder 与 API-key coding plans 真实账号/订阅验收仍 pending；Web Session 保持 Experimental/live-pending 且 tools/images 不开放；Cursor 仍为 Experimental/live-unverified；Bedrock planned；DeepSeek Web 明确 unsupported；Copilot github.com/GHES 与 Kiro 真实账号验收仍 pending |
| Gemini | `/v1beta/*` | Gemini/Gemini CLI、Antigravity/Agy、OpenRouter、Claude Messages、Codex Responses、Ollama、Nvidia、DeepSeek API key；GitHub Copilot Gemini→Chat 的 non-stream/stream、tool、usage 与单终态已 fixture-verified Native；Kimi Chat bridge、权威空目录与错误分离、transient-only exact-generation stale、无静态 entitlement fallback、K3 effort、auth/token 代际 thinking replay 与同账号单次 401 已 fixture-verified；Qoder COSY Gemini bridge、动态 site×route effort/context、签名时钟/UUID nonce、精确 account-generation model/session/quota scope、SSE terminal 与同账号单次 pre-commit replay 已 fixture-verified；Cursor Gemini emitter 与签名 Share `/v1beta/models` 精确 S2 Provider scope、跨 scope 隔离、权威空目录已 fixture-verified | Copilot github.com/GHES、Kimi 与 Qoder 真实账号验收仍 pending；Cursor 仍为 Experimental/live-unverified，需真实 stream/tool/image 证据；Bedrock planned；DeepSeek Web、Kiro 与 Grok/Perplexity Web Session 均不在 Gemini Surface 开放，保持 unsupported |

## 每个组合必须覆盖

- [ ] non-stream 成功响应。
- [ ] stream 成功响应。
- [ ] upstream 4xx 错误透传。
- [ ] upstream 5xx 或超时映射。
- [ ] 客户端取消或流中断。
- [ ] tool/function calling。
- [ ] image/media input。
- [ ] reasoning/thinking。
- [ ] cache read/write usage；断言 fresh input、cache read、cache creation、output 四桶不重叠，且总量为四桶之和。
- [ ] final usage 统计。
- [ ] request log：requestId、shareId、source、requestedModel、actualModel、status、latency、tokens。

## Direct / Market 维度

| 调用来源 | 必填环境变量 | 验收点 |
| --- | --- | --- |
| Router Share URL | `CC_SWITCH_SHARE_URL`、`ROUTER_API_TOKEN` | 同一 URL 上的 Claude/Codex/Gemini 请求均由 Router 鉴权，server 按签名 Share binding 执行且 server/router log 不重复 |

App-specific 变量优先级：

- Router Share：Claude、Codex、Gemini 三种协议统一使用 `CC_SWITCH_SHARE_URL`，不再配置每个 app 独立 URL。

## 推荐命令

```bash
scripts/smoke/code-agent-regression.sh
scripts/smoke/router-share-smoke.sh
node scripts/smoke/code-agent-matrix-summary.mjs
```

真实 stream 回归：

```bash
RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/code-agent-regression.sh
STREAM_PROBE=1 scripts/smoke/router-share-smoke.sh
MATRIX_LIVE_EVIDENCE_FILE=/private/code-agent-live-evidence.json REQUIRE_STREAM_USAGE=1 RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/code-agent-regression.sh
```

无真实 provider/token 时，`scripts/smoke/code-agent-regression.sh` 会运行 proxy、account domain、OAuth client、Web UI 和协议审计合同，以及可用的本地 server capability 检查；Share/Gateway/real provider 请求会输出 skipped 或 warning，不标记真实成功。每个 Rust 过滤器会先执行 `--list` 并强制要求至少命中一条测试。

stream 分支统一使用 `scripts/smoke/stream-probe.mjs`，只保存状态码、首块耗时、chunk/byte 计数、done/usage 标记和最多 2KB preview，不保存完整 stream 响应。默认要求看到结束事件；`REQUIRE_STREAM_USAGE=1` 时才把 usage 标记作为硬通过条件。

## 记录模板

```text
date:
server commit:
router:
market:
app:
provider type:
provider account/token source: redacted
entry path:
source: local/direct/market (`direct` 表示 Router Share 流量，不表示 Server 直连)
stream: true/false
request id:
status:
latency:
usage:
server log:
router log:
market log:
notes:
```
