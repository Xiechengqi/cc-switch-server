# Code Agent 回归矩阵

本矩阵对应 Z3/Z8。真实 provider/token 不存在时，只能执行本地 contract 和 mock 回归；真实账号回归不得伪标完成。

AD3 已将本页矩阵固化为 `docs/code-agent-regression-matrix.json`。`scripts/smoke/code-agent-regression.sh` 会先运行 `scripts/smoke/code-agent-matrix-summary.mjs`，输出：

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

`staticCoverage` 只表示本地 contract 和 fixture 已覆盖；`adapterStatus: mixed` 仍会被计入 mixed/skeleton，直到真实 provider、direct URL 和 market URL 都有 non-stream/stream 证据。

| App 入口 | 路径 | 已有 native/static contract | 仍未真实关闭 |
| --- | --- | --- | --- |
| Claude | `/v1/messages` | Claude API/Auth/OAuth、Codex Responses、Gemini/Gemini CLI、OpenRouter、Antigravity/Agy、Ollama、Nvidia、DeepSeek API key；Kiro Claude→CodeWhisperer bridge + native refresh 已接线但 capability 仍 planned；GitHub Copilot static preflight 已覆盖 model normalization、`/chat/completions` endpoint 和 optimizer headers/body 处理 | Cursor AgentService text/image/tool driver 默认接线，固定单凭据且包含 tool_result park-resume，maturity 保持 Experimental/live-unverified；Bedrock 只有 SigV4/Converse request parts；GitHub Copilot、Kiro、DeepSeek account 仍需真实验收 |
| Codex Responses | `/v1/responses` | Codex/OpenAI-compatible、OpenRouter、Ollama、Claude Messages、Gemini/Gemini CLI、Antigravity/Agy、Nvidia、DeepSeek API key；GitHub Copilot static OpenAI Chat preflight 已接入但 capability 仍不升级 | Cursor AgentService 默认接线、固定单凭据、Experimental/live-unverified；Bedrock planned；GitHub Copilot、Kiro、DeepSeek account 仍是 fallback/manual；Kiro server forwarding 当前 Claude-only |
| Codex Chat | `/v1/chat/completions` | 与 Codex Responses 同一 provider family；保留本入口用于回归 Chat->Responses normalization；GitHub Copilot static OpenAI Chat preflight 已接入但 capability 仍不升级 | Cursor AgentService 默认接线、固定单凭据、Experimental/live-unverified；Bedrock planned；GitHub Copilot、Kiro、DeepSeek account 仍是 fallback/manual；Kiro server forwarding 当前 Claude-only |
| Gemini | `/v1beta/*` | Gemini/Gemini CLI、Antigravity/Agy、OpenRouter、Claude Messages、Codex Responses、Ollama、Nvidia、DeepSeek API key；GitHub Copilot static OpenAI Chat preflight 已接入但 capability 仍不升级 | Cursor AgentService 默认接线、固定单凭据、Experimental/live-unverified；Bedrock planned；GitHub Copilot、Kiro、DeepSeek account 仍是 fallback/manual；Kiro server forwarding 当前 Claude-only |

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
| local share binding | `SERVER_URL`、`CC_SWITCH_INFERENCE_TOKEN`、`SHARE_ID` | inference token 通过且 server 能按 `X-CC-Switch-Share-Id` 命中 binding |
| direct public share URL | `DIRECT_SHARE_URL`、`ROUTER_API_TOKEN` | router auth 通过，server/router log 不重复 |
| market API URL | `MARKET_API_URL`、`ROUTER_API_TOKEN` | market -> router -> server -> provider 调度成功 |

App-specific 变量优先级：

- local：`CLAUDE_SHARE_ID`、`CODEX_SHARE_ID`、`GEMINI_SHARE_ID`；Codex 可回退到 `SHARE_ID`。
- direct：`DIRECT_CLAUDE_SHARE_URL`、`DIRECT_CODEX_SHARE_URL`、`DIRECT_GEMINI_SHARE_URL`；Codex 可回退到 `DIRECT_SHARE_URL`。
- market：`MARKET_CLAUDE_API_URL`、`MARKET_CODEX_API_URL`、`MARKET_GEMINI_API_URL`；Codex 可回退到 `MARKET_API_URL`。

## 推荐命令

```bash
scripts/smoke/code-agent-regression.sh
scripts/smoke/router-market-smoke.sh
node scripts/smoke/code-agent-matrix-summary.mjs
```

真实 stream 回归：

```bash
RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/code-agent-regression.sh
STREAM_PROBE=1 scripts/smoke/router-market-smoke.sh
MATRIX_LIVE_EVIDENCE_FILE=/private/code-agent-live-evidence.json REQUIRE_STREAM_USAGE=1 RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/code-agent-regression.sh
```

无真实 provider/token 时，`scripts/smoke/code-agent-regression.sh` 会运行 proxy、account domain、OAuth client、Web UI 和协议审计合同，以及可用的本地 server capability 检查；direct/market/real provider 请求会输出 skipped 或 warning，不标记真实成功。每个 Rust 过滤器会先执行 `--list` 并强制要求至少命中一条测试。

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
source: local/direct/market
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
