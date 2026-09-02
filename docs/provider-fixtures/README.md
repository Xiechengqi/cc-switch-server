# Provider Fixture 目录

本目录只保留说明文档。Provider 运行时身份以 `assets/contract/provider-registry.json` 为准；产品覆盖要求位于 `assets/contract/server-provider-requirements.json`，兼容窗口内的最小 preset fixture 位于 `assets/contract/provider-legacy-compatibility.json`。这些文件均由本仓库维护，不从外部工作树生成。

约束：

- 新增结构只按 Server reader、writer 和 runtime contract 的实际需求补充，并在 review 中说明消费路径。
- fixture 用于 adapter contract test、provider type 分类回归、usage parser snapshot。
- OAuth/账号型 provider 没有真实凭据时，只能保存脱敏配置结构和协议样例，不能标记真实登录能力完成。

OpenAI OAuth/Codex 的当前可执行协议样例保存在 `assets/contract/openai-oauth-protocol.json`。它固定官方 OAuth/上游地址、CLI callback、workspace header、可信 claim 合并样例和 WebSocket fallback 边界；Rust 单测直接消费 identity 与 fallback status 样例，修改实现或证据时必须同步更新并 review。

Claude OAuth 的当前脱敏 wire capture 保存在 `assets/contract/claude-oauth-wire-profile.json`。它只保留 Claude Code/Stainless/Node/Axios 公开版本、endpoint identity family、token endpoint 顺序、billing fingerprint/CCH/beta 合同、静态模型 ID，以及不含账号或业务内容的合成 CCH golden body；token、账号标识、真实/原始请求响应 body 与未验证的私有 build metadata 明确排除。Rust 的 `ClaudeWireProfile`、Messages/CountTokens beta 矩阵、profile/bootstrap/roles 请求身份和静态模型目录以该 capture 为共同证据，任一项变化都必须同步更新 fixture、实现、测试和 `PROTOCOL_EVIDENCE.md`。

Claude Max 20x 的本地 resolver 测试包含脱敏的 `default_claude_max_20x` 协议形状；5x 仅有同形解析规则。两者都不是 live credential evidence，仍必须分别通过 `scripts/smoke/claude-oauth-real.mjs` 的真实账号 gate 才能标记真实通过。

Kiro 的机器可读协议合同保存在 `assets/contract/kiro-wire-protocol.json`。它固定单一显式账号、同账号 401 最多一次刷新和一次重放、不可覆盖的生产 inference endpoint、Claude/Codex 下游协议、严格 AWS EventStream、tool `stop=true` 出流门禁、图片预算、账号代际模型缓存、prompt-cache namespace 和稳定错误码；明确排除账号池、轮询、跨账号目录并集与故障转移。`src/proxy/kiro.rs`、`src/proxy/kiro/{endpoint,image,wire}.rs` 和 `src/clients/oauth/kiro_runtime.rs` 的 Rust 测试直接消费该 fixture。本地状态仅为 `fixture_verified`，真实账号仍为 `live_pending`。

Qoder 的机器可读 oracle 保存在 `assets/contract/qoder-cli-oracle.json`。schema v2 以独立固定 digest 记录官方 Global/CN CLI `1.1.32`、`cli2api` 只读 capture projection、三条 credential rail、Device lifecycle、精确 COSY origin/path/profile/header、encoding/signature vector、去随机化完整 server body、payload/catalog 差分、quota、EOF terminal、验证计数与脱敏 receipt schema。`scripts/audit/audit-qoder-cli-oracle.mjs` 禁止外部路径、敏感材料、旧 refresh endpoint、coherent fixture mutation 和未解释漂移；`src/clients/oauth/qoder.rs`、`src/clients/oauth/quota.rs`、`src/proxy/qoder.rs` 与 `src/proxy/qoder_runtime.rs` 的 Rust 测试直接消费相同 fixture。loopback lifecycle/differential/harness 只建立 `fixture_verified`，三条真实 receipt 齐备前保持 `live_pending`。
