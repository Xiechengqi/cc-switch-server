# Provider Fixture 目录

本目录只保留说明文档。Provider 运行时身份以 `assets/contract/provider-registry.json` 为准；产品覆盖要求位于 `assets/contract/server-provider-requirements.json`，兼容窗口内的最小 preset fixture 位于 `assets/contract/provider-legacy-compatibility.json`。这些文件均由本仓库维护，不从外部工作树生成。

约束：

- 新增结构只按 Server reader、writer 和 runtime contract 的实际需求补充，并在 review 中说明消费路径。
- fixture 用于 adapter contract test、provider type 分类回归、usage parser snapshot。
- OAuth/账号型 provider 没有真实凭据时，只能保存脱敏配置结构和协议样例，不能标记真实登录能力完成。

OpenAI OAuth/Codex 的当前可执行协议样例保存在 `assets/contract/openai-oauth-protocol.json`。它固定官方 OAuth/上游地址、CLI callback、workspace header、可信 claim 合并样例和 WebSocket fallback 边界；Rust 单测直接消费 identity 与 fallback status 样例，修改实现或证据时必须同步更新并 review。

Codex 的一次性外部差异基线保存在 `assets/contract/codex-reference-delta.json`。它固定 Unicode schema/纯 const union、Responses HTTP/SSE/WS error/sequence/首尾帧、具名 tool output 与 WS→HTTP pre-commit 边界；同时把 GPT Image 2.5 三个 variant、WS prewarm 和后续图片 quota/cache 明确保持为逐项 `live_pending` 门禁。默认审计只读取本仓库；`node scripts/audit/audit-codex-reference-delta.mjs --check-sources` 才会按冻结 commit 和 SHA-256 可选复核外部 Git object。

Cursor 的一次性外部差异基线保存在 `assets/contract/cursor-reference-delta.json`。它只引用 OmniRoute 提交 `a3ca33fa6442b59adc42976c795709eaf5351109` 中的已提交 Git object，并冻结 ServerConfig/interaction 未知字段、重复字段失败关闭、Connect 分帧与 partial EOF、终态 envelope 和完整 fresh `*-fast` 模型 ID。`scripts/audit/audit-cursor-reference-delta.mjs` 默认只检查本地合同，`--check-sources` 才按 SHA-256 复核只读外部 object。`scripts/smoke/cursor-real.mjs` 一次只验收 OAuth 或 API-key rail，并只接受仓库外私密、scope digest-only 的 receipt；loopback fixture 不会把任一 rail 升级为 live verified。

Antigravity 的一次性外部差异基线保存在 `assets/contract/antigravity-reference-delta.json`。它冻结 `CLIProxyAPI` 的 reasoning replay、schema subset、transport scope 和 compaction 风险证据，以及 `Antigravity-Manager` 的 conversation session 样本；默认审计只检查本仓库，`node scripts/audit/audit-antigravity-reference-delta.mjs --check-sources` 才按 SHA-256 读取外部已提交 Git object。AG-01..03 为本地 `fixture_verified`；两条 rail 与 AG-04 compaction 仍为 `live_pending`，compaction 运行时固定关闭。

Grok 的一次性外部差异基线保存在 `assets/contract/grok-reference-delta.json`。它只读取 `grok2api@8913b53fe92307a6f111b2885ab298a43c74a9ba` 的七个已提交 Git object，并记录 reasoning replay/recovery 与 root-union 的八个历史 commit；外部工作树和 `sub2api` 路由实现均不进入依赖。默认审计只检查本仓库，`node scripts/audit/audit-grok-reference-delta.mjs --check-sources` 才按 SHA-256 复核外部 object。GR-01..03 为本地 fixture verified；推理、媒体与 remote compaction receipt 分离，GR-04/05 在真实输入缺失时保持 `live_pending`，remote compaction 的运行时开关固定关闭。

Claude OAuth 的当前脱敏 wire capture 保存在 `assets/contract/claude-oauth-wire-profile.json`。它只保留 Claude Code/Stainless/Node/Axios 公开版本、endpoint identity family、token endpoint 顺序、billing fingerprint/CCH/beta 合同、静态模型 ID，以及不含账号或业务内容的合成 CCH golden body；token、账号标识、真实/原始请求响应 body 与未验证的私有 build metadata 明确排除。Rust 的 `ClaudeWireProfile`、Messages/CountTokens beta 矩阵、profile/bootstrap/roles 请求身份和静态模型目录以该 capture 为共同证据，任一项变化都必须同步更新 fixture、实现、测试和 `PROTOCOL_EVIDENCE.md`。

Claude Max 20x 的本地 resolver 测试包含脱敏的 `default_claude_max_20x` 协议形状；5x 仅有同形解析规则。两者都不是 live credential evidence，仍必须分别通过 `scripts/smoke/claude-oauth-real.mjs` 的真实账号 gate 才能标记真实通过。

Kiro 的机器可读协议合同保存在 `assets/contract/kiro-wire-protocol.json`，一次性外部差异基线保存在 `assets/contract/kiro-reference-delta.json`。它固定单一显式账号、同账号 401 最多一次刷新和一次重放、不可覆盖的生产 inference endpoint、Claude/Codex 下游协议、严格 AWS EventStream、tool `stop=true` 出流门禁、图片预算、账号代际模型缓存和稳定错误码；prompt-cache 进一步固定 top-level auto/显式四断点、20-position lookback、tool group、混合 TTL、按条目续期、权威 usage 零值优先和整数比例守恒。持久化使用请求路径外的有界 writer 与 generation-CAS SQLite，旧 JSON 只导入且保留，remote/shared store 固定关闭。`src/proxy/kiro.rs`、`src/proxy/kiro_prompt_cache.rs`、`src/proxy/kiro/{endpoint,image,wire}.rs` 和 `src/clients/oauth/kiro_runtime.rs` 的 Rust 测试直接消费这些 fixture。`node scripts/audit/audit-kiro-reference-delta.mjs --check-sources` 才会复核 `kiro.rs@22d2c2d0695ba350890072c19990f54782827ae5` 的只读 Git object。KI-01..03 本地为 `fixture_verified`；按四类 auth kind × 两个 region 拆分的真实 receipt 仍全部 `live_pending`，KI-05 不会导入参考的 Redis、affinity 或账号路由。

Qoder 的机器可读 oracle 保存在 `assets/contract/qoder-cli-oracle.json`。schema v2 以独立固定 digest 记录官方 Global/CN CLI `1.1.32`、`cli2api` 只读 capture projection、三条 credential rail、Device lifecycle、精确 COSY origin/path/profile/header、encoding/signature vector、去随机化完整 server body、payload/catalog 差分、quota、EOF terminal、验证计数与脱敏 receipt schema。`assets/contract/qoder-reference-delta.json` 另把 revision 2、Registry/coverage/生产入口、TokenRouter 只读文档证据、CLI 升级顺序和 14 项真实验收门禁闭合为跨源映射。对应审计禁止外部路径成为依赖、敏感材料、旧 refresh endpoint、coherent fixture mutation 和未解释漂移；`src/clients/oauth/qoder.rs`、`src/clients/oauth/quota.rs`、`src/proxy/qoder.rs` 与 `src/proxy/qoder_runtime.rs` 的 Rust 测试直接消费相同 fixture。loopback lifecycle/differential/harness 只建立 `fixture_verified`；happy-path real harness 也会把未实际观察的登录/轮换、权威空目录和两段 401 标成 `not_observed`，不会单独把 rail 提升为 live verified。三条完整真实 receipt 齐备前保持 `live_pending`。

CodeBuddy revision 2 的差分资产为 `assets/contract/codebuddy-reference-delta.json`。它固定 `cli2api@e5893f0` 的空 WorkBuddy message 与 `@32aa108` 的空 chat-stream delta 证据哈希，映射本地 CB-01/02/03 落点，并将 Intl/CN 的 16 项真实检查分别保持为 `live_pending`。`scripts/audit/audit-codebuddy-reference-delta.mjs --check-sources` 可选验证外部 Git object，但外部仓库不进入构建或运行时。
