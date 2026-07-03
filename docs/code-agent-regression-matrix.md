# Code Agent 回归矩阵

本矩阵对应 Z3/Z8。真实 provider/token 不存在时，只能执行本地 contract 和 mock 回归；真实账号回归不得伪标完成。

AD3 已将本页矩阵固化为 `docs/code-agent-regression-matrix.json`。`scripts/code-agent-regression.sh` 会先运行 `scripts/code-agent-matrix-summary.mjs`，输出：

- `matrixTotal`：矩阵组合总数。
- `matrixRunnable`：当前环境变量齐备、可运行的组合数。
- `matrixSkipped`：缺少真实输入而跳过的组合数。
- `matrixSkeleton`：仍包含 skeleton/mixed adapter 的组合数。
- `staticNativeFamilies`：本地静态 adapter contract 已覆盖的 provider family。
- `staticPlannedFamilies`：已有请求计划或签名契约，但仍未启用真实转发的 provider family。
- `staticRemainingFallbackFamilies`：仍是 skeleton/manual/import-only 的 provider family。

这些字段会写入 acceptance evidence；没有真实 token 时只能说明 contract pass + real skipped，不能说明真实 provider 已通过。

## 入口维度

`staticCoverage` 只表示本地 contract 和 fixture 已覆盖；`adapterStatus: mixed` 仍会被计入 mixed/skeleton，直到真实 provider、direct URL 和 market URL 都有 non-stream/stream 证据。

| App 入口 | 路径 | 已有 native/static contract | 仍未真实关闭 |
| --- | --- | --- | --- |
| Claude | `/v1/messages` | Claude API/Auth/OAuth、Codex Responses、Gemini/Gemini CLI、OpenRouter、Antigravity/Agy、Ollama、Nvidia、DeepSeek API key；GitHub Copilot static preflight 已覆盖 model normalization、`/chat/completions` endpoint 和 optimizer headers/body 处理 | Cursor AgentService opt-in text/image/tool driver exists for Claude/Codex/Gemini, including MCP/built-in tool bridge and tool_result park-resume, but capability remains planned until real Cursor validation；Bedrock 只有 SigV4/Converse request parts；GitHub Copilot 仍需真实 token/live models/non-stream/stream 验收，Kiro、DeepSeek account 仍是 fallback/manual |
| Codex Responses | `/v1/responses` | Codex/OpenAI-compatible、OpenRouter、Ollama、Claude Messages、Gemini/Gemini CLI、Antigravity/Agy、Nvidia、DeepSeek API key；GitHub Copilot static OpenAI Chat preflight 已接入但 capability 仍不升级 | Cursor AgentService opt-in text/image/tool driver exists for Claude/Codex/Gemini, including MCP/built-in tool bridge and tool_result park-resume, but capability remains planned until real Cursor validation；Bedrock planned；GitHub Copilot、Kiro、DeepSeek account 仍是 fallback/manual |
| Codex Chat | `/v1/chat/completions` | 与 Codex Responses 同一 provider family；保留本入口用于回归 Chat->Responses normalization；GitHub Copilot static OpenAI Chat preflight 已接入但 capability 仍不升级 | Cursor AgentService opt-in text/image/tool driver exists for Claude/Codex/Gemini, including MCP/built-in tool bridge and tool_result park-resume, but capability remains planned until real Cursor validation；Bedrock planned；GitHub Copilot、Kiro、DeepSeek account 仍是 fallback/manual |
| Gemini | `/v1beta/*` | Gemini/Gemini CLI、Antigravity/Agy、OpenRouter、Claude Messages、Codex Responses、Ollama、Nvidia、DeepSeek API key；GitHub Copilot static OpenAI Chat preflight 已接入但 capability 仍不升级 | Cursor AgentService opt-in text/image/tool driver exists for Claude/Codex/Gemini, including MCP/built-in tool bridge and tool_result park-resume, but capability remains planned until real Cursor validation；Bedrock planned；GitHub Copilot、Kiro、DeepSeek account 仍是 fallback/manual |

## 每个组合必须覆盖

- [ ] non-stream 成功响应。
- [ ] stream 成功响应。
- [ ] upstream 4xx 错误透传。
- [ ] upstream 5xx 或超时映射。
- [ ] 客户端取消或流中断。
- [ ] tool/function calling。
- [ ] image/media input。
- [ ] reasoning/thinking。
- [ ] cache read/write usage。
- [ ] final usage 统计。
- [ ] request log：requestId、shareId、source、requestedModel、actualModel、pricingModel、status、latency、tokens。

## Direct / Market 维度

| 调用来源 | 必填环境变量 | 验收点 |
| --- | --- | --- |
| local share binding | `SERVER_URL`、`CC_SWITCH_SERVER_TOKEN`、`SHARE_ID` | server 能按 `X-CC-Switch-Share-Id` 命中 binding |
| direct public share URL | `DIRECT_SHARE_URL`、`ROUTER_API_TOKEN` | router auth 通过，server/router log 不重复 |
| market API URL | `MARKET_API_URL`、`ROUTER_API_TOKEN` | market -> router -> server -> provider 调度成功 |

App-specific 变量优先级：

- local：`CLAUDE_SHARE_ID`、`CODEX_SHARE_ID`、`GEMINI_SHARE_ID`；Codex 可回退到 `SHARE_ID`。
- direct：`DIRECT_CLAUDE_SHARE_URL`、`DIRECT_CODEX_SHARE_URL`、`DIRECT_GEMINI_SHARE_URL`；Codex 可回退到 `DIRECT_SHARE_URL`。
- market：`MARKET_CLAUDE_API_URL`、`MARKET_CODEX_API_URL`、`MARKET_GEMINI_API_URL`；Codex 可回退到 `MARKET_API_URL`。

## 推荐命令

```bash
scripts/code-agent-regression.sh
scripts/router-market-smoke.sh
node scripts/code-agent-matrix-summary.mjs
```

真实 stream 回归：

```bash
RUN_REAL=1 STREAM_PROBE=1 scripts/code-agent-regression.sh
STREAM_PROBE=1 scripts/router-market-smoke.sh
REQUIRE_STREAM_USAGE=1 RUN_REAL=1 STREAM_PROBE=1 scripts/code-agent-regression.sh
```

无真实 provider/token 时，`scripts/code-agent-regression.sh` 只跑 proxy/account contract 和可用的本地 server capability 检查；direct/market/real provider 请求会输出 skipped 或 warning，不标记真实成功。

stream 分支统一使用 `scripts/stream-probe.mjs`，只保存状态码、首块耗时、chunk/byte 计数、done/usage 标记和最多 2KB preview，不保存完整 stream 响应。默认要求看到结束事件；`REQUIRE_STREAM_USAGE=1` 时才把 usage 标记作为硬通过条件。

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
