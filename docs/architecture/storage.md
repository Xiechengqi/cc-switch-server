# 本地存储与数据目录

> 状态：**权威文档**。最后核对：2026-08-20。
>
> 本目录下的所有文件都可能含 token、密钥或用户数据，**任何情况下都不得提交到 git**。

## 1. 数据目录

- 默认位置：用户目录下 `.cc-switch-server`
- 覆盖方式：环境变量 `CC_SWITCH_SERVER_CONFIG_DIR`
- 目录权限：`0700`；进程持有独占文件锁 `.cc-switch-server.lock`（`0600`）

同一数据目录同时只能被一个进程使用。离线迁移命令在服务运行时会因拿不到锁而直接失败，错误信息提示先停止 `cc-switch-server`。实现见 `src/infra/storage.rs`。

## 2. 文件清单

| 路径 | 内容 | 敏感 |
| --- | --- | --- |
| `server.json` | 管理员密码 hash、owner email、Router 配置与密钥、client tunnel 子域、`requestBodyLimits` | 是 |
| `providers.json` | Provider 配置与分类后的 ProviderType；S1 / S2 两种格式 | 是 |
| `accounts.json` | Provider 账号与 OAuth 凭据 | 是 |
| `accounts.key` | 凭据根密钥 | 是 |
| `shares.json` | Share 定义、绑定、限额与授权 | 是 |
| `tunnels.json` | 隧道状态 | 是 |
| `email-auth.json` | 邮箱验证码登录状态 | 是 |
| `provider-health.json` | Provider 健康采样 | 否 |
| `model-pricing.json` | 模型定价表 | 否 |
| `grok-media-tasks.json` | Grok 媒体任务队列 | 否 |
| `usage/` | 用量账目（见 §4） | 部分 |
| `image-capabilities/` | 图像能力探测结果 | 否 |
| `backups/` | 备份归档（见 §5） | 是 |
| `.cc-switch-server.lock` | 数据目录独占锁 | 否 |

`store.json` 与 `.codex-workspace-rebind-transaction.json` 为迁移/事务过程文件，不是稳定契约。

> `AGENTS.md` 提醒：不要把这些文件的存在误判为最终 DB 迁移完成；SQLite 兼容与旧 cc-switch DB 读取必须另行设计和验收。

## 3. 凭据加密

实现见 `src/infra/credentials.rs` 与 `src/domain/providers/store_v2.rs`。

- 根密钥文件 `accounts.key`，也可由环境变量 `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY` 提供（base64，标准与 URL-safe 两种编码都接受）。
- 根密钥按用途派生出两把子密钥，info 串固定：
  - Provider：`cc-switch-server/provider-credentials/v1`
  - Account：`cc-switch-server/account-credentials/v2`
- 密码学算法：**XChaCha20-Poly1305**（24 字节 nonce）。
- `provider_key_id()` 由 Provider 子密钥派生出稳定标识，用于判断密文是否与当前密钥匹配。

`cc-switch-server config print` 输出的是脱敏摘要：**不得**打印密码 hash、API token hash、Router 私钥、`control_secret` 或任何 Provider / 账号 token。

## 4. 用量存储

实现见 `src/domain/usage/store.rs`，目录 `usage/`：

| 文件 | 说明 |
| --- | --- |
| `manifest.json` | 元数据，`USAGE_SCHEMA_VERSION = 1` |
| `requests.json` | 明细快照 |
| `events/YYYY-MM-DD.jsonl` | 事件日志，`USAGE_JOURNAL_VERSION = 1` |
| `rollups.json` | 聚合桶 |

关键常量：

- 明细保留 `USAGE_DETAIL_RETENTION_DAYS = 32` 天
- 聚合桶粒度 `USAGE_ROLLUP_BUCKET_MS = 60_000`（1 分钟）
- 每 `USAGE_COMPACT_EVERY_EVENTS = 500` 个事件触发一次压实

计量口径与字段语义见 [`usage-accounting.md`](usage-accounting.md)；Share 维度的用量重基线见 [`../share/user-usage-rebase.md`](../share/user-usage-rebase.md)。

## 5. 备份

实现见 `src/infra/backup.rs`：

- 目录 `backups/`，每个备份带 `manifest.json`
- 默认保留 `DEFAULT_BACKUP_KEEP = 24` 份，超出由 `prune_backups` 清理
- 恢复为两阶段：先 stage 再 validate，校验不过不落地
- 目录 `0700`、文件 `0600`

对应 API：`GET/POST /api/backup`（别名 `/api/backups`）、`POST /api/backup/:id/restore`。

## 6. 写入规则

跨存储写操作必须按字段声明顺序取锁：

```
config → providers → accounts → usage → shares → ui_settings → sessions → oauth_logins
```

新代码禁止在 `state.rs` 之外对 `ServerStateInner` 的存储字段 `.write().await` 后直接改数据。shares 写路径只允许经由 `mutate_shares_immediate` / `try_mutate_shares_immediate` / `mutate_shares_debounced` / `mutate_share` / `replace_shares` / `validate_share_invocation`。完整规则见 [`../../AGENTS.md`](../../AGENTS.md)。

## 7. 迁移

跨版本数据迁移步骤见 [`../guide/data-migration.md`](../guide/data-migration.md)。Provider 存储格式迁移实现在 `src/domain/providers/storage_migration.rs`；历史 Token Market 数据的一次性清理在 `src/domain/sharing/legacy_token_market_migration.rs`（仅历史清理，不是可用能力）。
