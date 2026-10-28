# 配置参考

> 状态：**权威文档**。最后核对：2026-09-19。
>
> 本文是配置项的完整清单。数据文件布局见 [`../architecture/storage.md`](../architecture/storage.md)。

默认配置目录为 `~/.cc-switch-server`。

## 1. 进程与监听

| 领域 | 配置 |
| --- | --- |
| 监听地址 | `--host` / `CC_SWITCH_SERVER_HOST`，默认 `127.0.0.1` |
| 监听端口 | `--port` / `CC_SWITCH_SERVER_PORT`，默认 `15721` |
| 配置目录 | `--config-dir` / `CC_SWITCH_SERVER_CONFIG_DIR`，默认 `~/.cc-switch-server` |
| 静态 Web | 默认使用构建时内嵌到 binary 的 Web UI；`--web-dist-dir` / `CC_SWITCH_SERVER_WEB_DIST_DIR` 仅用于开发或调试时覆盖静态目录 |
| 日志级别 | `--log-level` / `CC_SWITCH_SERVER_LOG`，默认 `info` |

## 2. 日志与审计

| 领域 | 配置 |
| --- | --- |
| 日志采集 | Web `设置 → 高级 → 日志管理` 中控制，默认开启；仅当本地日志开启且级别为 `info` 时记录请求生命周期、Provider 选择、重试/切换和终态等脱敏结构化 INFO 审计事件，并通过 installation 身份签名批量上传到当前 Router；不上传请求/响应正文、凭据、邮箱或任意 tracing 文本 |
| 持久日志 | `<config-dir>/log/cc-switch-server.log` 跨进程保留供本机诊断；单文件达到 8 MiB 后轮转，并在同目录保留最近 2 个备份，不按日志时间清理 |
| 审计日志缓冲 | `<config-dir>/log/audit-events.jsonl` 是 Router 上传前的本地 spool；单文件达到 16 MiB 后轮转并保留最近 7 个备份，上传端按 cursor 增量读取且 batch 不跨 boot stream，cursor 通过私有临时文件写入、`fsync`、原子替换和父目录同步持久化；Router 不可用时按上限留存并指数退避重试；spool writer 遇到临时文件错误会保留待写事件并自动退避恢复，队列或 writer 不可用时新的推理请求保持 fail-closed，已接纳请求的 terminal 事件即使采集开关随后关闭或 writer 暂时降级仍会排队等待恢复，恢复后补记脱敏状态事件；规范化 Router API 地址或 installation 任一变化，以及 cursor 缺失或损坏时，都会先隔离既有 backlog，避免日志跨 Client 或跨 Router 泄露 |
| Prometheus | `GET /metrics` 暴露账号并发、通用 retry/failover、Codex WS cache/fallback、Responses Lite/metadata/routing/文本 keepalive、Codex 限流作用域、Previous Response cache、图片 capability/心跳/静默时间、Provider outcome、warm-refresh 和版本闸门指标；公网部署需在反向代理层限制访问 |

升级任务向 Router 回报状态时，`/metrics` 额外暴露两个低基数指标，用于发现 Client 已完成升级但 Router 状态未收敛的问题：

- `cc_switch_router_upgrade_task_reports_total{outcome="success|failure"}`：升级任务回执尝试次数。
- `cc_switch_router_upgrade_task_report_last_success_timestamp_seconds`：最近一次成功回执的 Unix 时间戳；尚未成功回执前不会输出该时间序列。

## 3. Router 与请求体上限

| 领域 | 配置 |
| --- | --- |
| Router 心跳 | `CC_SWITCH_SERVER_ROUTER_HEARTBEAT_INTERVAL_SECS`，默认 `60` 秒，实际发送间隔带 ±10% jitter（允许范围 `15`-`60` 秒） |
| 请求体上限 | Router ingress 请求的生效上限是 `min(本地上限, Router 声明上限)`；Router 通过不参与签名的 `x-cc-switch-ingress-body-limit`（十进制字节）声明本次档位，伪造只能压低不能抬高。本地三档由 `server.json` 的 `requestBodyLimits.{defaultMb,mediaMb,imageMb}` 或 `CC_SWITCH_REQUEST_BODY_LIMIT_MB` / `CC_SWITCH_MEDIA_REQUEST_BODY_LIMIT_MB` / `CC_SWITCH_IMAGE_REQUEST_BODY_LIMIT_MB` 配置（普通档 1-64 MB，视频/图片档 1-256 MB 且不低于普通档），默认取上限 64/256/256 MB，即默认由 Router settings 决定实际天花板；改动需重启进程。旧版 Router 不发送该头时回退到历史值（普通 2 MiB / 视频 32 MiB / 图片 48 MiB） |
| 请求生命周期内存预算 | Codex HTTP 请求、每个 Codex Responses WebSocket turn，以及精确绑定到 `antigravity_oauth` / `agy_oauth` / `claude_oauth` / `cursor_oauth` / `cursor_apikey` / `grok_oauth` / `kiro_oauth` / `amazon_q_oauth` / `qoder_cosy` / `codebuddy_oauth` 的请求使用独立的 resident-memory budget；普通 `claude`、`claude_auth` 与 `gemini_cli` 不会因 App 相同而启用。通用核算原始/解压/规范化 body、transport pending、语义 prelude 与工具参数；Codex 另核算规范化事件和 WS 队列，Antigravity 另核算 reasoning replay 的 cache snapshot/流 accumulator 以及 grounding/citation/web-search bridge 状态，Claude OAuth 另核算压缩 SSE wire 与 decoded body、Anthropic/terminal/usage decoder pending state、tool partial JSON、工具名补丁和 stream transform retained state，Cursor 双 rail 另核算 Agent plan/protobuf、gzip expansion、h2 parser/pending frame、图片、SSE/JSON 聚合、semantic retry 与跨请求 parked session retained state；Grok OAuth 另核算 HTTP/WS reasoning replay clone/parse/apply/stream accumulator、SSE search/terminal retained state、媒体 multipart/base64 规范化、请求/响应解压和图片 JSON/SSE stream buffer；Kiro/Amazon Q 另核算 canonical/prepared/wire 副本、目标 headers、图片 base64/像素 decode/resize/RGB/JPEG 工作集、AWS EventStream frame/decoder、tool JSON/leak filter、prompt-cache 临时状态及下游 stream transform；Qoder COSY 另核算 canonical/runtime/catalog/prepared payload、plain JSON 与编码/签名工作副本、目标 headers、错误响应、SSE decoder/canonical event、非流聚合和下游 bridge retained state；CodeBuddy OAuth 另核算 canonical/runtime/catalog/prepared payload、JSON wire、请求 ID、目标 headers、错误 body/解析工作集、SSE decoder/canonical event owner、非流 content/reasoning/tool-call 聚合和三 Surface 下游 bridge retained state。由 `requestBodyLimits.memoryBudgetMb` 或 `CC_SWITCH_REQUEST_MEMORY_BUDGET_MB` 配置，范围 16-1024 MiB、默认 256 MiB；耗尽返回稳定的 HTTP 503（`Retry-After: 1`、`cc_switch_request_memory_exhausted`）或已提交 SSE 的 503 error terminal（Responses WS 为 error + Close），取消当前上游且不触发 replay/换账号，也不记为普通网络故障；Cursor continuation 会先向新请求预算预留再释放旧 reservation，Kiro/Amazon Q、Qoder 与 CodeBuddy 的同账号 401 replay 前后继续使用同一 sticky budget。指标只含组件、结果和字节数，不含请求内容或身份；改动需重启进程 |
| 进程保留字节父预算 | `CC_SWITCH_PROCESS_MEMORY_BUDGET_MB` 默认 `0`，只观测现有请求 reservation、Cursor completed-response/token/cooldown/model/endpoint 缓存、single-flight 与 continuation claim 独立 owner，Grok replay/CAS、媒体任务快照/持久化、模型目录与 OIDC/JWKS 缓存，Kiro 本地 Prompt Cache 条目/dirty与in-flight批次/发布计划scratch及独立模型/IDE缓存，CodeBuddy 目录snapshot与有界discovery single-flight，以及 Codex quota 队列、模型目录、previous-response 缓存、活动/空闲 WS 队列与连接缓存、referral 会话直接保留对象的逻辑字节；正整数为独立硬上限（MiB），首次使用时固定。先测部署并发与 RSS 再设置；它不是完整的进程 RSS 限制，尚未覆盖的独立缓存需按类别接入。父/子增长整体成功，失败后请求仍 sticky exhausted，缩容及最后 owner drop 始终归还额度；进程拒绝保持既有 503 capacity 合同，不触发刷新、账号健康处罚或透明重放。`cc_switch_process_resident_memory_*` 提供 used/high-water/limit/active reservations/rejections；无身份或内容标签 |
| Codex 被动额度观察 | `CC_SWITCH_CODEX_QUOTA_OBSERVER=0`（或 `false` / `off`）关闭新观察，生成及权威额度查询继续工作；默认开启，ServerState 加载时固定，更改需重启。HTTP quota 头与审核过的 WS metadata/rate-limit 帧只投影固定窗口、plan 和完整 credits 布尔对，不记录 routing/Authorization/model headers。最多 64 个排队身份、每项保守计 8 KiB，100ms 合并窗口、持久化最多三次尝试；读取帧时不等待数据库。记录与推理 token/Share 用量分开，按 Provider revision、账号 incarnation/auth/token generation 及每字段时间拒绝晚到数据 |
| 图片内容层上限 | 请求体档位之外，图片内容另有独立的语义上限，且**返回 400 而非 413**：multipart `/v1/images/edits` 每张 20 MiB、合计 32 MiB、最多 16 张（`src/proxy/remote_image.rs`）。因此 multipart 图片编辑的实际天花板恒为 32 MiB，把 Router / 本地图片档调到 32 MiB 以上不会放宽它。JSON / data-URL 形式的 `/v1/images/edits` 只校验 16 张上限。Claude / Cursor 通道内联远程图片 URL 时单张上限 1 MiB |
| 媒体响应上限 | 响应侧上限独立于请求体档位且不可配：Grok 媒体响应 64 MiB（超限 502）、Codex Images 输出 48 MiB / 上游 72 MiB / 8294400 像素。视频结果偏大时先撞的是这一条，不是请求体上限 |

进程父预算还覆盖 Antigravity replay/CAS 与目录、Qoder session/目录/flight/continuation 及 Claude 线程别名。共享 session/目录/continuation 的 owner 跟随最后引用，缓存删除不等于数据已释放；Qoder 可选 alias 缓存拒绝仍保留本次普通响应成功，后续 required continuation 会严格缺失。Cursor 下游取消记 `client_cancelled`，不计 Provider 网络失败；parked 完成和真正的上游失败保持原分类。这些逻辑估值不包含所有控制面解析或 HTTP/TLS 内部内存，不能用来推导部署 RSS 硬阈值。

协议细节见 [`../architecture/router-contract.md`](../architecture/router-contract.md)。

## 4. OAuth 与账号

| 领域 | 配置 |
| --- | --- |
| OAuth client | Gemini 浏览器登录需要 `CC_SWITCH_SERVER_GEMINI_CLIENT_ID` / `CC_SWITCH_SERVER_GEMINI_CLIENT_SECRET`；Antigravity/Agy 浏览器登录需要 `CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_ID` / `CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_SECRET` |
| Managed OAuth 并发 | 每账号默认最多 8 个 in-flight 请求；provider 可设置 `ACCOUNT_MAX_CONCURRENT` / `MAX_CONCURRENT_REQUESTS`，全局可用 `CC_SWITCH_ACCOUNT_MAX_CONCURRENT` 覆盖，设为 `0` 关闭 |
| OAuth 重登隔离 | 连续 20 次 `invalid_grant` 后账号自动标记为需重登并退出其固定 Provider 内的账号调度；`CC_SWITCH_REFRESH_FAILURES_BEFORE_RELOGIN` 可调整阈值 |
| 凭据根密钥 | `accounts.key` 文件，或 `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY`（32 字节 base64） |

## 5. 转发运行时

| 领域 | 配置 |
| --- | --- |
| Streaming 超时 | Provider 默认首业务事件超时 120 秒、后续事件空闲超时 300 秒；`STREAM_FIRST_BYTE_TIMEOUT_MS` / `UPSTREAM_STREAM_FIRST_BYTE_TIMEOUT_MS` 和 `STREAM_IDLE_TIMEOUT_MS` / `UPSTREAM_STREAM_IDLE_TIMEOUT_MS` 可覆盖，设为 `0` 关闭对应超时 |
| Claude OAuth cache | billing block 固定不带 `cache_control`；identity/prompt block 默认保持 CLI 兼容的 5 分钟 TTL，`CC_SWITCH_CLAUDE_CACHE_TTL=1h` 可启用 1 小时 prompt cache |
| Claude OAuth CLI identity | 默认使用审计 profile 的 Claude Code `2.1.280`，`.env.example` 不再写死 override。`CC_SWITCH_CLI_UA_VERSION` 仅接受不低于内建 profile 的有界三段数字版本；`CC_SWITCH_CLI_UA` 仅接受以不低于内建 profile 的 `claude-cli/<三段数字版本>` 开头、无控制字符且长度有界的完整 UA，合法完整 UA 优先。两者仅用于上游版本门禁的紧急覆盖；无效或旧候选会被忽略，没有任何合法覆盖时 fail closed 到固定 profile。启动日志和 `cc_switch_claude_wire_profile_info` 会显示 effective version、来源与 `stale_override_rejected` |
| Codex WebSocket cache | 默认最多缓存 64 条空闲连接，idle TTL 5 分钟、max age 55 分钟；`CC_SWITCH_CODEX_WS_CACHE_MAX_CONNECTIONS`、`CC_SWITCH_CODEX_WS_CACHE_IDLE_MS`、`CC_SWITCH_CODEX_WS_CACHE_MAX_AGE_MS` 可覆盖，provider 的 `codexWebsocketEnabled=false` 可紧急关闭 WS |
| Codex overflow compact | `CC_SWITCH_CODEX_OVERFLOW_AUTO_COMPACT=1` 可在业务输出提交前对 `context_length_exceeded` 使用同账号做一次有界摘要和重试；默认关闭，摘要调用会单独计入 usage |
| Codex Responses keepalive | `CC_SWITCH_CODEX_RESPONSES_KEEPALIVE_MS` 控制普通文本 Responses SSE 在下游已收到首个业务/终态事件后的注释心跳，默认 `15000` ms；`0` 禁用，非零值收敛到 `5000..60000` ms。Provider `driverOptions.codexResponsesKeepaliveIntervalMs` 优先于环境变量。心跳不提交首包，也不延长 `STREAM_FIRST_BYTE_TIMEOUT_MS` / `STREAM_IDLE_TIMEOUT_MS` |
| Codex Responses SSE normalizer | `CC_SWITCH_CODEX_RESPONSES_SSE_NORMALIZER_ENABLED` 默认开启，对 OpenAI OAuth 上游 Responses SSE/JSON/NDJSON 做有界严格解析、liveness 白名单过滤和 canonical SSE 重编码；`0` 只用于事故回滚。回滚后不再保证坏 200 body 在下游提交前被拦截 |
| 出站 HTTP/2 PING | `CC_SWITCH_OUTBOUND_HTTP2_KEEPALIVE_ENABLED` 默认关闭；开启后 `CC_SWITCH_OUTBOUND_HTTP2_KEEPALIVE_INTERVAL_MS` 默认 `45000`、限制 `10000..300000`，`CC_SWITCH_OUTBOUND_HTTP2_KEEPALIVE_TIMEOUT_MS` 默认 `20000`、限制 `5000..60000`。只维持活动 HTTP/2 transport，不改变业务 first-event/idle/terminal timeout |
| Codex routing hint | `CC_SWITCH_CODEX_ROUTING_HINT_ENABLED` 默认 `false`；Provider `driverOptions.codexRoutingHintEnabled` 可覆盖。开启后只从最终 HTTP body 的最终模型和已验证 `priority` tier 合成 Server 独占 `x-codex-routing-hint`；客户端/account 同名 header 会被删除或拒绝，WebSocket handshake 永不携带该 hint。真实 OAuth 验收前保持关闭 |
| Codex Images | capability URL 固定使用 Router 签名 context 中的 Share host；短期图片默认保存到 `<config-dir>/image-capabilities`，仅多副本共享时用 `CC_SWITCH_IMAGE_STORE_DIR` 覆盖，底层文件系统必须支持跨进程锁和 atomic rename |

## 6. 验收相关变量

| 领域 | 配置 |
| --- | --- |
| 真实验收 | `ROUTER_BASE_URL`、`ROUTER_API_TOKEN`、`SHARE_ID`、`CC_SWITCH_SHARE_URL` 及各真实 Provider token |
| stream 验收 | `STREAM_PROBE`、`REQUIRE_STREAM_USAGE` |
| release readiness | `RUN_TESTS`、`RUN_REAL`、`RUN_DEPLOYMENT_TESTS` |
| 回归矩阵路径 | `MATRIX_PATH`，默认 `docs/provider/regression-matrix.json` |

占位符清单见仓库根目录 `.env.example`。**真实值不得提交。**

## 7. Provider 存储格式

全新数据目录在首次保存 Provider 时写入带格式 guard 的 S2 `providers.json`，静态 API Key、Bearer、AWS secret 和受控自定义 header 值以 XChaCha20-Poly1305 credential slot 保存。已有 S1 安装会继续读取 S1，普通启动不会改写文件；管理员必须停服后显式切换：

```bash
# 可在服务运行时只读预检
cc-switch-server config migrate-provider-store

# 以下写操作必须先停止 cc-switch-server
cc-switch-server config migrate-provider-store --apply
cc-switch-server config migrate-provider-store --rollback
cc-switch-server config migrate-provider-store --cleanup-snapshot
```

`--apply` 只有在身份/凭据分类及 S1/S2 RuntimePlan parity 全部通过时才会创建 S1 快照并切换；`--rollback` 恢复该快照；快照只通过显式 `--cleanup-snapshot` 删除。服务持有数据目录锁时，三个写操作均会失败。

如果配置了 `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY`，恢复和迁移时必须提供完全相同的 32 字节 base64 根密钥；否则必须保留匹配的 `accounts.key`。S2 能防止单独泄露 `providers.json` 或不含密钥的 Provider 快照，但完整数据目录、环境根密钥或 Server OS 用户权限一并泄露时仍可解密，**不能把它视为硬件密钥边界**。

## 8. 数据目录迁移

跨环境迁移必须先停止旧、新实例，再完整复制实际配置目录；不能只复制部分 JSON。具体步骤和 OAuth 加密密钥要求见 [`data-migration.md`](data-migration.md)。
