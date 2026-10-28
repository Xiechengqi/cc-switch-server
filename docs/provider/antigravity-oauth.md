# Antigravity / Agy OAuth

状态：权威。两条 rail 分别使用 `antigravity_oauth` / `special.antigravity` 和 `agy_oauth` / `special.agy`，每个 Provider 固定绑定一个 Account；账号、凭据和目录缓存互不借用。Claude 与 Gemini 为已登记入口，真实接受性继续按各 rail 的私有回执独立验收。

## 目录与客户端版本

`fetchAvailableModels` 在请求开始时捕获客户端版本、完整 User-Agent 和不透明 generation。缓存键和实际 HTTP 头使用这一份快照，并保留账号 incarnation、auth/token generation、Bearer 指纹、project 和 endpoint 隔离。运行时版本更新失败保持原快照；有效版本发生变化时更换 generation，A→B→A 也不复活 A 的旧观察。`CC_SWITCH_ANTIGRAVITY_CLIENT_VERSION` 仍可固定有效客户端版本。

成功空目录是权威结果。只有网络、408、429、5xx 可以使用同一完整身份的旧目录；新版本失败不能继承上一版本目录。缓存仍有 24 小时 stale 上限和 128 项容量限制，401/403、坏成功响应与身份漂移失败关闭。

目录产生的能力观察持久化 `antigravityClientIdentity`，发布时检查当前客户端身份，旧请求晚到不能发布到新 generation。旧数据缺少该字段、版本变更或进程重启后，目录观察显示 `stale/superseded`，须重新探测才恢复 fresh。stale 回退不延长能力观察有效期。项目、隐私与独立 quota 探测继续遵守各自的凭据及有效期合同。

搜索模型选择与实际出站 UA 共用 ProviderExecution 捕获的身份。请求已捕获 A、等待期间更新到 B 时，它仍按 A 的证据与 UA 执行；其返回的容量错误不能写成 B 的新观察。Server 不把缺少目录解释为所有模型都获授权。

## 多轮签名与工具

回放仍要求相同 Provider、账号 incarnation/凭据、caller、会话、模型、endpoint 和语义历史；TTL、容量、终态与拒绝后的 CAS 清理继续有效。前置 signed thought 或纯签名 carrier 在客户端省略思考、保留 unsigned function call 时，只回填到同轮首个匹配调用；并行调用不复制签名，调用名称、参数、ID 或上下文变化不能借用旧签名。已有对应思考段时按原段回填。

流式工具身份优先使用上游显式 ID。不同 chunk 复用同一个 part 位置但携带不同 ID 时，分别保留调用，并分配连续的下游工具索引；同一显式 ID 的参数分片继续合并。无 ID 调用仍遵守已有分片合同，完整调用与参数分片之间的歧义须取得独立 wire 证据后再调整。

工具声明和历史不授予本轮执行权限。`tool_choice=none` 仍发送 Gemini `NONE`，历史调用与其签名可保留。Claude 模型委托到 Antigravity 时遵守 Gemini wire 合同；原生 Claude OAuth 的 Opus 5.5 强制工具拒绝规则不据模型名称直接套用到委托链路。委托的模型/thinking/工具组合尚需真实目录和厂商接受回执，离线合同测试不提升模型能力。

## 本地转发时间观察（2026-10-27）

两条 rail 分别使用固定的 `antigravity_oauth` / `agy_oauth` 指标标签，覆盖已登记的 Claude、Gemini App 流/非流和独立 Gemini `countTokens` operation。request 从本地转发入口开始，各 attempt 独立计时；API 收包/鉴权不在此时段。准入、发出、headers、首语义内容、body handoff、终态、最后已核算 request owner 释放分别记录，原 `first_token_ms`、用量、deadline 和恢复判定保持。

非流式的首内容在上游 SSE 聚合期间观察，终态等待原有聚合验证；流式沿既有解码与最终结算。空签名 carrier、空文本、usage、停止原因和心跳不算首语义内容；有效零计数也算结果。`MAX_TOKENS` 只影响当前 attempt 的 incomplete 观察，blocked prompt 可没有首内容。Google RPC 短延迟恢复的原有两处等待分别计时，未扩大重试范围。HTTP body handoff 不代表 socket 写入成功或远端消费；指标不携带账号、会话、模型字符串、正文或签名。此项不证明厂商接受性、部署资源上限或性能收益。

## 证据与门禁

本轮来源为 CLIProxyAPI `1f917e77c4562623aa49e9804d999524151a2f96` 和 Antigravity-Manager `b601f5e7375343343922ccab5e50653bc3450c3b` 的只读提交对象。取舍与本仓 fixture 记录在 [reference delta](../../assets/contract/antigravity-reference-delta.json) 的独立 `r4Reviews`；旧 observation 和 receipt schema 不改写。可选 `--check-sources` 审核外仓对象，默认构建、测试及运行时不读取外仓。

R4 首轮 wire 增强的离线执行范围是包含 Claude/Qoder/Antigravity 改动的隔离工作树，排除了当时并行写入的 Share Market 修改。该范围已写入 R4 review 的 `validationScope`；该次完整后端回归不代表并行 Market 改动或真实接受性已通过。后续 CORE-03 使用当前主仓冻结快照，其独立验证与同步状态见增强计划 §20.10.2，不改写早期成绩的范围。

Manager 的模块职责可作设计参考；不迁入其追加“继续”再生成并拼接输出的 auto-heal，也不整体替换本仓 IR。Compact、未验证模型、两条 rail 的真实接受性继续保持原门禁。验收入口：`RUN_REAL=0 node scripts/smoke/antigravity-real.mjs --rail antigravity_oauth`（另一条 rail 为 `agy_oauth`）。输入不足只记录 `blocked_inputs/live_pending`。
