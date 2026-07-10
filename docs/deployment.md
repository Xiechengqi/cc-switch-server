# cc-switch-server 部署说明

`cc-switch-server` 目标是单 binary + config dir 长期运行。

## 本地验证

静态受限场景（不编译、不部署、不启动服务）：

```bash
scripts/static-checks.sh
```

完整本地验证：

```bash
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
scripts/audit/validate-local.sh
scripts/smoke/smoke-local.sh
MODE=binary scripts/smoke/deployment-smoke.sh
RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=1 scripts/release-readiness.sh
```

`validate-local.sh` 固定执行：

```bash
cargo fmt --check
cargo check
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
cargo test
```

## CLI 运维命令

`cc-switch-server` 无子命令时默认启动 HTTP server；也可以显式使用 `serve`：

```bash
cc-switch-server serve --host 0.0.0.0 --port 15721
```

部署前或排障时优先使用只读命令：

```bash
cc-switch-server config path
cc-switch-server config print
cc-switch-server config validate
cc-switch-server doctor
```

`config print` 只输出脱敏摘要。`config validate` 和 `doctor` 不启动 HTTP server、router 注册、SSH tunnel 或后台监听器。需要同时检查端口可绑定时使用：

```bash
cc-switch-server doctor --check-port
```

## systemd

参考 `deploy/cc-switch-server.service`。生产环境建议显式设置：

- `--host 0.0.0.0`
- `--port 15721`
- `--config-dir /var/lib/cc-switch-server`
- `--web-dist-dir /opt/cc-switch-server/web-dist`

常用命令：

```bash
sudo install -m 0755 target/release/cc-switch-server /usr/local/bin/cc-switch-server
sudo install -m 0644 deploy/cc-switch-server.service /etc/systemd/system/cc-switch-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now cc-switch-server
sudo journalctl -u cc-switch-server -f
```

升级和回滚：

1. 停止服务：`sudo systemctl stop cc-switch-server`
2. 备份旧 binary：`sudo cp /usr/local/bin/cc-switch-server /usr/local/bin/cc-switch-server.bak`
3. 安装新 binary 并启动：`sudo install -m 0755 target/release/cc-switch-server /usr/local/bin/cc-switch-server && sudo systemctl start cc-switch-server`
4. 如需回滚：`sudo cp /usr/local/bin/cc-switch-server.bak /usr/local/bin/cc-switch-server && sudo systemctl restart cc-switch-server`

Web 管理端的一键升级使用同文件系统 staging 和持久 rollback：

- staging：`/usr/local/bin/.cc-switch-server.new`
- rollback：`/usr/local/bin/cc-switch-server.bak`
- 任务状态：`<config-dir>/upgrade-state.json`

下载 binary 后必须通过 release `.sha256` 校验和 `--help` sanity check。systemd 部署通过独立 transient helper 原子替换 binary，重启后检查 `/version` 的 commit；检查失败会恢复 rollback。standalone 模式只终止当前 PID，不使用进程名全局 kill。容器内默认禁用一键升级，必须发布并部署新 image。

Client Tunnel 下所有非登录类 `/web-api/*` 都由 Router 先做 owner/admin 鉴权。SSE 使用带 `Authorization` 的 fetch stream，不允许把 access token 放入 query string。

## Docker

示例：

```bash
docker build -t cc-switch-server .
docker run -d --name cc-switch-server \
  -p 15721:15721 \
  -v cc-switch-server-data:/data/cc-switch-server \
  cc-switch-server
```

容器健康检查应访问宿主暴露的 `/health`，或在编排系统里配置 HTTP healthcheck：

```yaml
healthcheck:
  test: ["CMD", "curl", "-fsS", "http://127.0.0.1:15721/health"]
  interval: 30s
  timeout: 5s
  retries: 3
```

## Router/Market 联调

在 server 已启动并登录拿到 bearer token 后：

```bash
CC_SWITCH_SERVER_TOKEN=... \
SERVER_URL=http://127.0.0.1:15721 \
SHARE_ID=share-id \
scripts/smoke/router-market-smoke.sh
```

脚本只通过 server/market HTTP API 探测，不修改 router、market 或 cc-switch 代码。

## TLS/反代

建议外层使用 Caddy/Nginx/Cloudflare Tunnel 终止 TLS，再反代到 `127.0.0.1:15721` 或内网地址。`router` tunnel 暴露的 public URL 与本机管理入口可以并存，但生产管理入口必须使用强密码和最小暴露面。

## 数据目录

配置目录包含：

- `server.json`
- `providers.json`
- `accounts.json`
- `accounts.key`
- `shares.json`
- `usage-logs.json`
- `tunnels.json`

这些文件使用原子写入方式保存。`accounts.json` 中的账号 token 字段会用 `accounts.key` 加密；也可以用 `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY` 提供 32 字节 base64 密钥。备份时直接备份整个 config dir，不能只备份 `accounts.json` 而漏掉 `accounts.key`。

备份恢复：

1. `sudo systemctl stop cc-switch-server`
2. `sudo tar czf cc-switch-server-config.tgz -C /var/lib cc-switch-server`
3. 恢复时解压到同一路径并确认权限属于服务用户。
4. `sudo systemctl start cc-switch-server`
5. 登录 Web 或调用 `/api/router/diagnostics` 检查 router/share/tunnel 状态。
