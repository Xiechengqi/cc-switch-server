# 配置参考

> 状态：**权威文档**。最后核对：2026-08-26。
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
| Prometheus | `GET /metrics` 暴露账号并发、通用 retry/failover、Codex WS cache/fallback、Responses Lite/metadata/routing/文本 keepalive、Previous Response cache、图片 capability/心跳/静默时间、Provider outcome、warm-refresh 和版本闸门指标；公网部署需在反向代理层限制访问 |

升级任务向 Router 回报状态时，`/metrics` 额外暴露两个低基数指标，用于发现 Client 已完成升级但 Router 状态未收敛的问题：

- `cc_switch_router_upgrade_task_reports_total{outcome="success|failure"}`：升级任务回执尝试次数。
- `cc_switch_router_upgrade_task_report_last_success_timestamp_seconds`：最近一次成功回执的 Unix 时间戳；尚未成功回执前不会输出该时间序列。

## 3. Router 与请求体上限

| 领域 | 配置 |
| --- | --- |
| Router 心跳 | `CC_SWITCH_SERVER_ROUTER_HEARTBEAT_INTERVAL_SECS`，默认 `60` 秒，实际发送间隔带 ±10% jitter（允许范围 `15`-`60` 秒） |
| 请求体上限 | Router ingress 请求的生效上限是 `min(本地上限, Router 声明上限)`；Router 通过不参与签名的 `x-cc-switch-ingress-body-limit`（十进制字节）声明本次档位，伪造只能压低不能抬高。本地三档由 `server.json` 的 `requestBodyLimits.{defaultMb,mediaMb,imageMb}` 或 `CC_SWITCH_REQUEST_BODY_LIMIT_MB` / `CC_SWITCH_MEDIA_REQUEST_BODY_LIMIT_MB` / `CC_SWITCH_IMAGE_REQUEST_BODY_LIMIT_MB` 配置（普通档 1-64 MB，视频/图片档 1-256 MB 且不低于普通档），默认取上限 64/256/256 MB，即默认由 Router settings 决定实际天花板；改动需重启进程。旧版 Router 不发送该头时回退到历史值（普通 2 MiB / 视频 32 MiB / 图片 48 MiB）。请求体整体驻留内存，收紧本地上限即可为本机内存兜底 |
| 图片内容层上限 | 请求体档位之外，图片内容另有独立的语义上限，且**返回 400 而非 413**：multipart `/v1/images/edits` 每张 20 MiB、合计 32 MiB、最多 16 张（`src/proxy/remote_image.rs`）。因此 multipart 图片编辑的实际天花板恒为 32 MiB，把 Router / 本地图片档调到 32 MiB 以上不会放宽它。JSON / data-URL 形式的 `/v1/images/edits` 只校验 16 张上限。Claude / Cursor 通道内联远程图片 URL 时单张上限 1 MiB |
| 媒体响应上限 | 响应侧上限独立于请求体档位且不可配：Grok 媒体响应 64 MiB（超限 502）、Codex Images 输出 48 MiB / 上游 72 MiB / 8294400 像素。视频结果偏大时先撞的是这一条，不是请求体上限 |

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
| Claude OAuth cache | billing/identity block 默认保持 CLI 兼容的 5 分钟 TTL；`CC_SWITCH_CLAUDE_CACHE_TTL=1h` 可启用 1 小时 prompt cache |
| Codex WebSocket cache | 默认最多缓存 64 条空闲连接，idle TTL 5 分钟、max age 55 分钟；`CC_SWITCH_CODEX_WS_CACHE_MAX_CONNECTIONS`、`CC_SWITCH_CODEX_WS_CACHE_IDLE_MS`、`CC_SWITCH_CODEX_WS_CACHE_MAX_AGE_MS` 可覆盖，provider 的 `codexWebsocketEnabled=false` 可紧急关闭 WS |
| Codex overflow compact | `CC_SWITCH_CODEX_OVERFLOW_AUTO_COMPACT=1` 可在业务输出提交前对 `context_length_exceeded` 使用同账号做一次有界摘要和重试；默认关闭，摘要调用会单独计入 usage |
| Codex Responses keepalive | `CC_SWITCH_CODEX_RESPONSES_KEEPALIVE_MS` 控制普通文本 Responses SSE 在下游已收到首个业务/终态事件后的注释心跳，默认 `15000` ms；`0` 禁用，非零值收敛到 `5000..60000` ms。Provider `driverOptions.codexResponsesKeepaliveIntervalMs` 优先于环境变量。心跳不提交首包，也不延长 `STREAM_FIRST_BYTE_TIMEOUT_MS` / `STREAM_IDLE_TIMEOUT_MS` |
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
