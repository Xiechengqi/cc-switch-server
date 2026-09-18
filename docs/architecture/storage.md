# 本地存储与数据目录

> 状态：**权威文档**。最后核对：2026-09-11。
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
| `server.json` | 管理员密码 hash、owner email、Router 配置与密钥、client tunnel 子域、`requestBodyLimits`（三档 ingress body 上限与单请求生命周期内存预算） | 是 |
| `providers.json` | 首次 SQLite 迁移前的 Provider 配置；提交后移入只读 migration backup | 是 |
| `accounts.json` | 首次 SQLite 迁移前的 Provider 账号与 OAuth 凭据；提交后移入只读 migration backup | 是 |
| `accounts.key` | 凭据根密钥 | 是 |
| `shares.json` | 首次 SQLite 迁移前的 Share 定义；提交后移入只读 migration backup | 是 |
| `tunnels.json` | 隧道状态 | 是 |
| `email-auth.json` | 邮箱验证码登录状态 | 是 |
| `provider-health.json` | Provider 健康采样 | 否 |
| `model-pricing.json` | 模型定价表 | 否 |
| `grok-media-tasks.json` | Grok 媒体任务队列 | 否 |
| `usage/` | 首次 SQLite 迁移前的用量账目；提交后移入只读 migration backup | 部分 |
| `server-store.sqlite3` | providers/accounts/shares/usage 的事务型权威库与加密回滚 blob | 是 |
| `server-store-migration.json` | SQLite schema、源摘要与 authority 状态 marker | 否 |
| `migration-backups/server-store-*/` | 权威切换时生成的一次性只读 legacy 回滚集 | 是 |
| `image-capabilities/` | 图像能力探测结果 | 否 |
| `backups/` | 备份归档（见 §5） | 是 |
| `.cc-switch-server.lock` | 数据目录独占锁 | 否 |

`store.json` 与 `.codex-workspace-rebind-transaction.json` 为迁移/事务过程文件，不是稳定契约。

首次启动构建并验证 `shadow_verified`。显式设置
`CC_SWITCH_SERVER_SQLITE_AUTHORITY=committed` 后，启动才经 `prepared` 原子推进到
`committed`；一旦进入 prepared/committed，后续启动会强制 roll-forward。marker 为
`committed` 时 SQLite 是唯一权威；根目录下重新出现的旧 JSON/JSONL 不会被反向导入，
并会在验证数据库后清理。`accounts.key` 始终留在数据库外。

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

迁移前的格式实现见 `src/domain/usage/store.rs`，目录 `usage/`：

| 文件 | 说明 |
| --- | --- |
| `manifest.json` | 元数据，`USAGE_SCHEMA_VERSION = 1` |
| `requests.json` | 明细快照 |
| `events/YYYY-MM-DD.jsonl` | 事件日志，`USAGE_JOURNAL_VERSION = 1` |
| `rollups.json` | 聚合桶 |

这些文件在 migration backup 中用于离线降级。committed 运行态改为
`usage_records` 单记录 UPSERT，以 `request_id` 为 CAS/幂等键，不再在请求热路径同步
重写快照或 JSONL。关键 legacy 常量：

- 明细保留 `USAGE_DETAIL_RETENTION_DAYS = 32` 天
- 聚合桶粒度 `USAGE_ROLLUP_BUCKET_MS = 60_000`（1 分钟）
- 每 `USAGE_COMPACT_EVERY_EVENTS = 500` 个事件触发一次压实

计量口径与字段语义见 [`usage-accounting.md`](usage-accounting.md)；Share 维度的用量重基线见 [`../share/user-usage-rebase.md`](../share/user-usage-rebase.md)。

## 5. 备份

实现见 `src/infra/backup.rs`：

- 目录 `backups/`，每个备份带 `manifest.json`
- 自动备份策略来自 `ui-settings.json`：默认每 12 小时执行一次并保留 3 份；`backupIntervalHours = 0`
  表示停用自动创建，但仍执行保留数清理。设置保存后运行时立即重新计算，无需重启
- 手动与自动备份都按当前 `backupRetainCount` 裁剪；服务启动时会立即清理超额备份，以及历史版本
  遗留的、符合 Server 生成 ID 格式但缺少 `manifest.json` 的不完整目录
- 新备份先写入同文件系统的私有 staging 目录，文件与 manifest 落盘后再原子 rename；创建失败不会
  暴露新的 `backup-*` 目录。SQLite 权威模式同时包含 `server-store.sqlite3` 与 migration marker
- 创建、列出、裁剪、删除、重命名与恢复由 `ServerState` 的备份协调器串行化，避免保留清理与
  pre-restore 安全备份并发修改同一目录
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

### 7.1 SQLite schema 与不变量

实现见 `src/repository/server_sqlite.rs`，当前 schema version 为 1：

- `providers` 主键为 `(app, provider_id)`，保留 revision、credential generation 和原始 S2 密文 record；
- `accounts` 主键为 `(provider_type, account_id)`，保留 auth identity/token refresh generation 和原始字段级密文；
- `provider_accounts`、`shares`、`share_bindings` 使用 foreign key 校验 Provider/Account/Share 图；
- `usage_records` 以 request id 为主键，按创建时间和 Share 建索引，payload 保留未知扩展字段；
- `legacy_blobs` 保存原始加密 JSON/JSONL 字节，供一个发布窗口内精确 DB→legacy 回滚；`accounts.key` **不进入 SQLite**，回滚仍必须同时持有数据目录根密钥。

所有时间写入 SQLite 时使用 Unix 毫秒并限制在 signed 64-bit 范围。所有 generation
必须非负且不会在导入时重编号。数据库开启 `foreign_keys=ON`、`trusted_schema=OFF`、
WAL、`synchronous=FULL`、5 秒 busy timeout 和 1000 page auto-checkpoint；这些参数是
schema v1 的明确崩溃合同，后续只能凭故障/性能数据修改。

### 7.2 Shadow import、切换与验证

Server 已持有数据目录独占锁后，启动会执行以下过程：

1. 读取 providers/accounts/shares/usage 源文件，按相对路径、长度和 SHA-256 生成总摘要；
2. 在随机临时路径创建 SQLite，单事务写入全部表；
3. 校验逐表计数、Provider/Account 关键 generation 哈希、foreign-key graph 与 `integrity_check`；
4. Account/Provider payload 只能取自磁盘密文，禁止序列化内存明文；非空 Provider S1 会拒绝导入，须先迁到 S2；
5. checkpoint、fsync 后原子替换影子数据库，原子写 `shadow_verified` marker；
6. 管理员停服后运行 `cc-switch-server config migrate-server-store --apply`，或显式设置
   `CC_SWITCH_SERVER_SQLITE_AUTHORITY=committed` 再启动，生成确定路径的只读 migration
   backup，依次持久化 DB/marker 的 `prepared` 与 `committed`；
7. 验证 committed DB 后移除根目录 legacy 源，后续启动只从 SQLite 重建内存 Store。

在临时库创建、事务提交或替换前失败不会改变上一个已验证数据库。任何切换失败都会
使启动 fail closed；已有 `committed` marker 时禁止从旧文件反向覆盖数据库。

### 7.3 权威切换与崩溃状态机

冻结的状态机为：

```text
legacy_authoritative + shadow_verified
  → prepared (DB、源摘要、回滚集全部验证)
  → committed (SQLite reader/writer 成为唯一权威)
```

- `prepared` 前的进程退出：删除临时文件或继续使用上一份 shadow；
- `prepared` 后的进程退出：必须按 marker/source digest 确定性 roll forward，不能双写猜测；
- `committed` 后：禁止重新从 legacy 导入，legacy 只存在于带时间戳的只读 migration backup；
- 降级/回滚：停服、取得数据目录锁、使用 `legacy_blobs` 加现有 `accounts.key` 导出到新目录，验证后再替换；
- SQLite 备份在既定锁序下冻结 writer、执行 WAL checkpoint，再纳入 manifest；仓储层同时
  提供 online backup API 给离线/故障验证，不复制未 checkpoint 的 WAL 主文件。
- Provider/Account/Share 写入分别使用短事务；跨三域的 Codex workspace rebind 使用单一
  `persist_reference_graph` 事务；Usage 热路径只 UPSERT 一条记录。

离线操作先停服，所有命令都会取得数据目录独占锁：

```bash
# 只读预检；不创建 DB 或 marker
cc-switch-server --config-dir "$CONFIG_DIR" config migrate-server-store

# 显式切换 SQLite 权威；重复执行可安全恢复或确认 committed
cc-switch-server --config-dir "$CONFIG_DIR" config migrate-server-store --apply

# 从当前 committed 权威导出到一个尚不存在的新目录
cc-switch-server --config-dir "$CONFIG_DIR" config migrate-server-store \
  --rollback-export /secure/new-legacy-directory
```

回滚导出会重新生成当前 Usage，而不是回放迁移时的旧 Usage blob，并复制物理
`accounts.key`。如果根密钥只由 `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY` 提供，离线旧版
进程也必须获得完全相同的环境密钥。导出不会替换当前数据目录；管理员应先在隔离目录
验证旧版读取，再停服执行受控目录切换。

### 7.4 故障注入门禁

迁移测试覆盖：临时库失败、事务失败、replace 前进程退出、旧 shadow/marker 保持、
密文无明文泄漏、online backup `integrity_check`、legacy export、重复迁移、source digest
漂移、foreign-key 破坏，以及 backup、DB prepared、marker prepared、DB committed、marker
committed、legacy retire 的每个崩溃点 roll-forward。staged restore 还会在系统临时目录中从
密文 rollback payload 只读重建 Provider/Account/Share，并核对逐表数量、generation hash 与
subscription reference graph；不会修改 stage。损坏 DB/WAL 必须 fail closed；committed 后
重新出现或损坏的 legacy 文件不得成为读取来源。
