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

## 2026-09-02 Claude Code 2.1.258 OAuth wire profile freeze

`claude_oauth` 的当前 wire profile 来自对官方 npm `@anthropic-ai/claude-code@2.1.258` native binary 的一次性静态审计，以及只连接本地 loopback、使用假凭据的出站请求捕获；审计过程没有访问 Anthropic，也没有保存或使用真实 access/refresh token。审计当日 npm `latest` / `next` 为 `2.1.258`，`stable` 仍为 `2.1.236`。发布漂移检查以 `latest` 为目标，同时只记录 `stable`；Server 构建和运行时都不访问 npm。

证据确认 Claude Code、Stainless、Node 与 Axios 的公开版本分别为 `2.1.258`、`0.112.1`、`v26.3.0` 与 `1.15.2`，并确认 `claude-fable-5-1` canonical model、mid-conversation system 能力、保留的 CCH 流程以及 prompt-derived billing suffix。billing suffix 以 salt `59cf53e54c78`、原始请求第一条 user text 的 JavaScript UTF-16 code unit 索引 4/7/20（缺位补 `0`）和有效 CLI version 计算 SHA-256，取前三位 hex；`ping` / `2.1.258` 的固定结果为 `1e2`。billing block 本身不带 `cache_control`，CCH 仍在所有 body rewrite 后生成。profile、算法常量和脱敏 golden 位于 `assets/contract/claude-oauth-wire-profile.json`，生产实现不读取外部二进制。

`sub2api` commit `34b8bf1a6` 只作为 Fable 5.1 目录和 billing fingerprint 概念的交叉证据；其 Go 实现按 UTF-8 byte 取索引，不能覆盖官方 JavaScript UTF-16 语义。审计时 TokenRouter/sub2api 仍广告 Claude Code `2.1.220`、Stainless `0.94.0`，且二者关于取消 CCH 的判断与当前官方 binary 不符，因此都没有作为版本、identity、CCH 或 beta 的实现来源。吸收范围不含其多账号号池、账号切换或 fallback 设计。

2026-09-03 对 TokenRouter `5f94cbcf2d1f4e74badf449c192c1431dc4e5c8e` 与 sub2api `6566039bc81e8a9af94077cb272eb3d3074702dd` 做了后续一次性只读差异审计。两者的 `backend/internal/service/ratelimit_service.go` 和 `account_usage_service.go` 交叉确认：Anthropic Messages 响应头会提供独立的 `5h`、`7d`、`7d_oi` utilization/reset，其中 `7d_oi` 是 Fable 周容量窗口；主动 usage 缺失该窗口时，可以用同账号成功推理得到的被动样本补齐显示。Server 只吸收这组有限 header 名称和“主动值优先、被动值补缺”的协议事实，并独立加入数值/时间范围校验、Account identity generation 隔离、单调合并、TTL/reset 过期和 Fable entitlement 门控。没有采用外部项目的 Extra map 存储、预测窗口、调度阈值、自动暂停、账号选择、号池或 fallback 行为；普通 utilization 也不会写入仅表示已耗尽的 `capacity_pool_limits`。

离线证据只能支持 `fixture_verified`。真实 Max 5x/20x inference、Fable 5.1 entitlement、streaming、限流和版本门禁仍为 `live_pending`，必须使用已轮换且不进入日志/命令历史的私密凭据按 acceptance runbook 验证后才能升级结论。

## 2026-09-02 Qoder CLI oracle freeze

`qoder_cosy` 的 native Rust 实现以一次性、只读的官方 CLI 审计作为漂移 oracle，不在构建或运行时加载 CLI。证据冻结于 `assets/contract/qoder-cli-oracle.json`：Global `@qoder-ai/qodercli@1.1.32` bundle SHA-256 为 `24de5b12520cbe49c0027b53654eaee02bddd857e3d9f19a6198824e365d89bf`，CN `@qodercn-ai/qoderclicn@1.1.32` bundle SHA-256 为 `5a82eeffbeb015d78c4945b7f4ed989494d2ea8cc7fdf2dbfc6ad04c17418f8b`。`cli2api` commit `9b18f2de06c53f12bf2c5112c7a71e3e64755b97` 仅提供带文件摘要的 capture/plaintext projection 交叉样本，不是依赖、同步源或生产 executor。

两份官方 bundle 共同确认：Device authorization 使用 `/device/selectAccounts`、UUID v4 nonce 与 S256；poll 为 OpenAPI `GET /api/v1/deviceToken/poll`，1 秒间隔、300 秒 TTL、404 pending 且不发送 Authorization/COSY/User-Agent；refresh 为 OpenAPI `POST /api/v1/deviceToken/refresh`，只发送 JSON body `refresh_token` 与 `User-Agent: qoder/1.1.32`，响应主字段为 `device_token`、轮换 `refresh_token`、`expires_at`。Global 36 位小写 hex machine ID 与 CN UUID v4 machine ID 是独立站点事实。Qoder CLI `1.1.32` 和 COSY wire `1.24.2` 属于不同版本空间；旧 Global center job-token endpoint 不可作为 Device refresh fallback。

oracle schema v2 额外用审计脚本内的独立摘要冻结三 rail 的精确 origin、actual/signature path、Global/CN profile、完整 signed-header 集、encoding/signature vectors、两侧 projection 与每条 accepted-difference 原因，因而不能通过同时修改 fixture 两侧来维持假绿。canonical synthetic Chat 同时保存去随机 UUID 后的完整 server body；Rust 对 Global/CN 生产 builder 生成的整棵 JSON 做 exact equality，并验证 signed-header 集没有额外字段。Global profile/session 只接受恰好 36 位小写 hex machine ID，CN 只接受 RFC 4122 variant UUID v4，错站、空白、大小写、version 与 variant 在发网前失败。

2026-09-03 的查漏补缺继续把外部仓库限制为只读交叉证据：TokenRouter `5f94cbcf2d1f4e74badf449c192c1431dc4e5c8e` 的 `qoder_gateway_service.go`（SHA-256 `1fd0f4b37b96c04927a6c3e9dc7a6711ed0bd3422eec1f7e100ab42f103b7d22`）与 `gateway_forward_as_chat_completions.go`（`25fe31a1ca48f74a49da43a3178cfd33ae34525a560a60b5239782240491af6b`）用于核对 response/tool envelope；cli2api `b67278960df9c160d45a7520ee3110b5ccb84126` 的 `worker/src/plaintext.mjs`（`1f25273ac8b8b7b156ea70945bcbedcca38007cc5fea55d7360fa2d4ca85a413`）、`sse.mjs`（`a3620d3bcf49674136c55b536a41d6f161ae2bc5513ff29fab2452d5462686a6`）与 `errors.mjs`（`d22ed21e6cbc1fe3e807de662760cb218e6f8943c7806c4118a8e76eb1d47650`）用于核对工具历史、SSE 与 Retry-After。吸收内容只包含单绑定账号内的 bounded compatibility、错误/脱敏和容量治理；没有吸收账号池、权重、轮询选号、跨账号/跨站 fallback，也没有采用“缺失终态仍成功”或旧 Center refresh 行为。文件摘要与安全不变量冻结在 oracle 的 `compatibilityPolicy`。

CN `cosy-clientip` 现由 Server 自身出站路由决定并在 catalog、quota、auth-status、generation 四条链路共用：目标路由优先，remote-DNS 情况下允许不发包的默认路由探测，运维可用 `CC_SWITCH_QODER_CN_CLIENT_IP` 提供规范 IPv4；任何下游 forwarded header 都不可信。Global 保持 machine-ID client IP 语义。响应兼容统一 OpenAI final-message/content-block/reasoning/usage 与 Anthropic-style content/tool/message envelope，工具 result 只在当前 batch 唯一关联；这些归一化不改变唯一 terminal + EOF 才成功的规则。

验证计数由 oracle 的 `verification` 单源维护：63 项 Rust Qoder 专项、9 项 Node mutation、7 项 loopback real-harness fixture；生成 coverage 直接读取该元数据。Node audit 还精确校验 lifecycle method/path/timing/header/body/response、三 rail 隔离、quota、bounded compatibility 与 EOF terminal；Rust 测试直接消费同一 oracle 的 lifecycle URL builder、payload/header/catalog、quota parser 与 SSE decoder。终态只有在唯一 authoritative terminal 后读到上游 EOF 才成立。`scripts/smoke/qoder-real.mjs` 的三条真实 rail receipt 仍未提供，因此这些加固只提高离线 wire 可信度，当前证据仍只允许 `fixture_verified` / `live_pending`，不得写成 live verified。

## 2026-09-01 CodeBuddy OAuth evidence freeze

`codebuddy_oauth` 的实现合同来自一次性、只读的协议研究，优先级如下：

1. CodeBuddy CLI `2.142.0` bundle、站点 overlay，以及国际个人订阅账号的脱敏真实流量；
2. 本仓库 [`docs/provider/codebuddy-oauth.md`](docs/provider/codebuddy-oauth.md) 已冻结的端点、OAuth、refresh、目录、计费与 terminal 约束；
3. `cli2api` commit `9b18f2d` 的 WorkBuddy CN/Global adapter，仅作为国内实现、payload、错误投影与缺陷的交叉样本。

实现不得在构建或运行时读取上述外部源码。国际站固定为 `https://www.codebuddy.ai`，国内站固定为 `https://copilot.tencent.com`；站点属于账号身份，不允许失败后换 host。CLI `2.142.0` 证据优先于 `cli2api` 使用的旧 `2.139.0` wire。国际 fixture 可据此标为离线通过；国内真实数据面、企业账号、图像/视频与本仓库真实订阅 receipt 仍是 `live_pending`。

2026-09-03 又对 `/data/projects/proxy/CodeBuddy/workbuddy-switch` 做了一次性只读差异审计，只吸收单账号控制面证据：`domain` 决定受控 billing origin，summary / paid / free 三路新资源接口及旧 `/v2` 回退，`X-Client-Platform: web`，逐请求用量分页与 prompt 字段白名单裁剪，以及闲置 refresh session 的 `12153` 终态。Server 独立实现将稳定身份冻结为 `site + uid + enterpriseId`，把 domain 降为同站可更新路由属性；任一路资源 401 交给已有单次 refresh/replay，官方用量只缓存安全投影。明确不吸收该项目的账号池/轮换、自动签到、进程与本地配置切换，也不吸收 prompt rewrite。企业账号和多模态继续 fail closed；未执行真实订阅验收，因此 registry 的 forward/test/discovery 仍保持 `live_pending`。

## 2026-09-01 Trae CN Solo evidence freeze

`trae_solo` 的 wire 来自对 `cli2api` commit `9b18f2d` 中 `internal/providers/trae/` 的一次性只读审计。采纳的事实仅限固定端点、Cloud-IDE 身份头、Solo payload、模型详情、订阅额度与 `metadata/output/token_usage/done/error` 事件结构；实现由本仓库独立完成，外部源码不是同步源或依赖。

固定出站 origin 为 OAuth `https://api.trae.com.cn`、Agent `https://trae-api-cn.mchost.guru`、Billing `https://api.trae.cn`、浏览器授权 `https://www.trae.cn`。明确拒绝参考实现中的三类行为：callback 消费任意 pending flow、导入的 `api_host` 控制凭据目的地、以及 EOF/error 后合成成功终态。Server-native Solo bridge 不等于 Trae IDE MITM、插件注入或桌面流量劫持。

离线 fixture 或 mock 只能证明合同接线；真实 OAuth、refresh、目录、额度、三 Surface 流式/tools/reasoning、401 恢复与错误码仍保持 `live_pending`，直到本仓库留存脱敏 receipt。
