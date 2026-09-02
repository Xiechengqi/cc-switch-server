# Protocol evidence policy

`cc-switch-server` 的 Provider 产品范围、身份和运行时行为由本仓库维护，当前权威来源是：

- `assets/contract/server-provider-requirements.json`：必须覆盖的 ProviderType 与 App Surface；
- `assets/contract/provider-registry.json`：Family、Profile、Driver、凭据与模型策略；
- `assets/contract/provider-legacy-compatibility.json`：兼容窗口内只读的 S1/旧 Web fixture；
- `assets/contract/*-protocol.json`、本仓库测试及厂商公开协议：具体 wire 行为证据。

外部仓库可以在一次明确的协议研究中作为只读差异证据，但不是实现同步源，也不能成为构建、测试、发布或运行时输入。吸收证据时必须在本仓库形成最小合同、独立实现与回归测试；证据不足的能力保持 `live_pending` 或 fail closed。

历史上从其他开源项目改编的代码与界面工作记录在 `SOURCE_PROVENANCE.json`，完整归属与许可证见 `THIRD_PARTY_NOTICES.md`。这些记录用于合规和溯源，不赋予外部仓库当前产品权威性，也不要求 CI checkout 外部源码。

Provider 合同变更至少运行：

```bash
node scripts/audit/audit-server-provider-contract.mjs
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
```

## 2026-09-02 Qoder CLI oracle freeze

`qoder_cosy` 的 native Rust 实现以一次性、只读的官方 CLI 审计作为漂移 oracle，不在构建或运行时加载 CLI。证据冻结于 `assets/contract/qoder-cli-oracle.json`：Global `@qoder-ai/qodercli@1.1.32` bundle SHA-256 为 `24de5b12520cbe49c0027b53654eaee02bddd857e3d9f19a6198824e365d89bf`，CN `@qodercn-ai/qoderclicn@1.1.32` bundle SHA-256 为 `5a82eeffbeb015d78c4945b7f4ed989494d2ea8cc7fdf2dbfc6ad04c17418f8b`。`cli2api` commit `9b18f2de06c53f12bf2c5112c7a71e3e64755b97` 仅提供带文件摘要的 capture/plaintext projection 交叉样本，不是依赖、同步源或生产 executor。

两份官方 bundle 共同确认：Device authorization 使用 `/device/selectAccounts`、UUID v4 nonce 与 S256；poll 为 OpenAPI `GET /api/v1/deviceToken/poll`，1 秒间隔、300 秒 TTL、404 pending 且不发送 Authorization/COSY/User-Agent；refresh 为 OpenAPI `POST /api/v1/deviceToken/refresh`，只发送 JSON body `refresh_token` 与 `User-Agent: qoder/1.1.32`，响应主字段为 `device_token`、轮换 `refresh_token`、`expires_at`。Global 36 位小写 hex machine ID 与 CN UUID v4 machine ID 是独立站点事实。Qoder CLI `1.1.32` 和 COSY wire `1.24.2` 属于不同版本空间；旧 Global center job-token endpoint 不可作为 Device refresh fallback。

oracle schema v2 额外用审计脚本内的独立摘要冻结三 rail 的精确 origin、actual/signature path、Global/CN profile、完整 signed-header 集、encoding/signature vectors、两侧 projection 与每条 accepted-difference 原因，因而不能通过同时修改 fixture 两侧来维持假绿。canonical synthetic Chat 同时保存去随机 UUID 后的完整 server body；Rust 对 Global/CN 生产 builder 生成的整棵 JSON 做 exact equality，并验证 signed-header 集没有额外字段。Global profile/session 只接受恰好 36 位小写 hex machine ID，CN 只接受 RFC 4122 variant UUID v4，错站、空白、大小写、version 与 variant 在发网前失败。

验证计数由 oracle 的 `verification` 单源维护：56 项 Rust Qoder 专项、8 项 Node mutation、7 项 loopback real-harness fixture；生成 coverage 直接读取该元数据。Node audit 还精确校验 lifecycle method/path/timing/header/body/response、三 rail 隔离、quota 与 EOF terminal；Rust 测试直接消费同一 oracle 的 lifecycle URL builder、payload/header/catalog、quota parser 与 SSE decoder。终态只有在唯一 authoritative terminal 后读到上游 EOF 才成立。`scripts/smoke/qoder-real.mjs` 的三条真实 rail receipt 仍未提供，因此这些加固只提高离线 wire 可信度，当前证据仍只允许 `fixture_verified` / `live_pending`，不得写成 live verified。

## 2026-09-01 CodeBuddy OAuth evidence freeze

`codebuddy_oauth` 的实现合同来自一次性、只读的协议研究，优先级如下：

1. CodeBuddy CLI `2.142.0` bundle、站点 overlay，以及国际个人订阅账号的脱敏真实流量；
2. 本仓库 [`docs/provider/codebuddy-oauth.md`](docs/provider/codebuddy-oauth.md) 已冻结的端点、OAuth、refresh、目录、计费与 terminal 约束；
3. `cli2api` commit `9b18f2d` 的 WorkBuddy CN/Global adapter，仅作为国内实现、payload、错误投影与缺陷的交叉样本。

实现不得在构建或运行时读取上述外部源码。国际站固定为 `https://www.codebuddy.ai`，国内站固定为 `https://copilot.tencent.com`；站点属于账号身份，不允许失败后换 host。CLI `2.142.0` 证据优先于 `cli2api` 使用的旧 `2.139.0` wire。国际 fixture 可据此标为离线通过；国内真实数据面、企业账号、图像/视频与本仓库真实订阅 receipt 仍是 `live_pending`。

## 2026-09-01 Trae CN Solo evidence freeze

`trae_solo` 的 wire 来自对 `cli2api` commit `9b18f2d` 中 `internal/providers/trae/` 的一次性只读审计。采纳的事实仅限固定端点、Cloud-IDE 身份头、Solo payload、模型详情、订阅额度与 `metadata/output/token_usage/done/error` 事件结构；实现由本仓库独立完成，外部源码不是同步源或依赖。

固定出站 origin 为 OAuth `https://api.trae.com.cn`、Agent `https://trae-api-cn.mchost.guru`、Billing `https://api.trae.cn`、浏览器授权 `https://www.trae.cn`。明确拒绝参考实现中的三类行为：callback 消费任意 pending flow、导入的 `api_host` 控制凭据目的地、以及 EOF/error 后合成成功终态。Server-native Solo bridge 不等于 Trae IDE MITM、插件注入或桌面流量劫持。

离线 fixture 或 mock 只能证明合同接线；真实 OAuth、refresh、目录、额度、三 Surface 流式/tools/reasoning、401 恢复与错误码仍保持 `live_pending`，直到本仓库留存脱敏 receipt。
