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
node scripts/audit/audit-proxy-bridge-contract.mjs --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
scripts/audit/validate-local.sh
scripts/smoke/smoke-local.sh
MODE=binary scripts/smoke/deployment-smoke.sh
RUN_TESTS=1 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=1 scripts/release-readiness.sh
```

`RUN_TESTS=0` 仅用于负向审计：脚本会记录 `local-contracts-unverified`，输出 `decision=blocked` 并以状态码 `1` 退出；不得将其作为本地合同或发布验收通过证据。

`validate-local.sh` 固定执行：

```bash
cargo fmt --check
cargo check
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
node scripts/audit/audit-proxy-bridge-contract.mjs --check
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

## CLI 初始化

服务未启动 HTTP 时，可直接写 `server.json`：

```bash
cc-switch-server init \
  --owner-email owner@example.com \
  --router-url https://sgptokenswitch.cc \
  --password-stdin
```

远程 HTTP 初始化（无需鉴权）：

```bash
scripts/bootstrap/server-init-http.sh
```

本机 CLI 初始化：

```bash
scripts/bootstrap/server-init-local.sh
```

服务启动后若尚未 setup，日志会打印浏览器、curl bootstrap、CLI init 三种方式的完整示例命令。

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

release binary 和 checksum 下载请求使用目标 commit 作为 cache key。下载后必须通过 release `.sha256`、`--help` 和 staged binary `version --json` commit 校验，全部成功后才允许停止当前服务，避免 mutable `latest` CDN 返回上一版资产。systemd 部署通过独立 transient helper 原子替换 binary，重启后检查 `/version` 的 commit；检查失败会恢复 rollback。standalone 模式只终止当前 PID，不使用进程名全局 kill。容器内默认禁用一键升级，必须发布并部署新 image。

普通重启与升级替换分开执行：systemd 部署通过延迟 transient unit 调用 `systemctl restart --no-block`；standalone/nohup 部署启动独立 helper，终止当前 PID 后从 `/proc/self/exe` 对应的实际 binary 路径恢复原启动参数。替代进程将 stdout/stderr 写入 `<config-dir>/log/server.log`，不依赖 `/usr/local/bin` 或 `/var/log` 权限；helper 自身输出写入 `<config-dir>/log/restart-helper.log`。管理页同时以 PID 和 `processInstanceId` 判断重启完成。

replacement helper 会把最后一次本机 `/version` probe 的连接、HTTP、JSON 或 commit mismatch 原因和 rollback 结果写入任务日志。Client Tunnel 在进程替换期间可能短暂返回 Router 404/503；Web 会持续按原 task ID 恢复 status，只有 replacement commit 通过校验才 reload，回滚则显示 failed 和 helper 诊断。

Client Tunnel 下所有非登录类 `/web-api/*` 都由 Router 先做 owner/admin 鉴权。SSE 使用带 `Authorization` 的 fetch stream，不允许把 access token 放入 query string。

Client/share SSH tunnel 通过签名的 `/v1/tunnels/lease/renew` 在原连接上续期。正常 lease 到期不会重建 SSH 或短暂删除 public route；续期网络错误和 Router 5xx 会保留当前连接并重试，只有身份、lease 或 route 归属等终态拒绝才回退到重新申请 lease。部署时应先升级 Router，再升级 Server；Server 遇到尚未支持续期接口的旧 Router 会按终态错误回退到旧的重连流程。

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

## Client/Router Share 联调

在 server 已启动并登录拿到 bearer token 后：

```bash
CC_SWITCH_SERVER_TOKEN=... \
SERVER_URL=http://127.0.0.1:15721 \
SHARE_ID=share-id \
scripts/smoke/router-share-smoke.sh
```

脚本只通过 Server/Router HTTP API 探测，不修改 Router、Server 或其他仓库代码。

## TLS/反代

建议外层使用 Caddy/Nginx/Cloudflare Tunnel 终止 TLS，再反代到 `127.0.0.1:15721` 或内网地址。`router` tunnel 暴露的 public URL 与本机管理入口可以并存，但生产管理入口必须使用强密码和最小暴露面。

Codex OAuth Images 穿过 Cloudflare 时，反代必须流式透传源站 Body，不能在 Worker 中调用 `.text()`、`.json()` 或 `.arrayBuffer()`。Capability URL 的 host 固定来自 Router 签名 Share context，不从源站 Host 或 forwarded header 推导。Capability 文件默认持久化到 `<config-dir>/image-capabilities`；多副本应把 `CC_SWITCH_IMAGE_STORE_DIR` 指向同一个支持跨进程文件锁、atomic rename 和目录同步的挂载目录，让 `/v1/images/files/<token>` 的 Router 鉴权 GET/HEAD 可落到任一副本。不能共享该目录时才配置生成与下载的粘性回源。Cloudflare/WAF 上传规则需允许 48 MiB Codex Images HTTP envelope。Images 响应和 capability 文件都必须保持 `no-store`；详细约束和 524 验收见 [`codex-oauth-single-account.md`](codex-oauth-single-account.md#cloudflare-proxy)。

## OAuth/代理桥接运维

`/api/accounts/capabilities` 的 `loginFlows` 是 OAuth 登录方式的权威能力列表；旧客户端可继续读取由它派生的 `supportsStartLogin` 和 `supportsCallback`。Claude OAuth 支持 browser/CLI manual callback，OpenAI OAuth 还支持 device code。该接口只描述 Server 已实现的控制面能力，不代表真实账号、上游配额或 Router callback 已完成验收。

跨协议 reasoning envelope 使用 accounts 根密钥经 HKDF 派生的独立 HMAC-SHA256 key。轮换 `accounts.key` 或 `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY` 会同时改变该验证 key；旧请求历史中的 Server envelope 将按 fail-closed 处理，不能把这一行为改成接受未认证 opaque 内容。tool schema、tool-result media、reasoning、Anthropic 请求合法化、stream lifecycle 和 Responses 失败语义的可执行基线位于 `assets/contract/proxy-bridge-protocol.json` 与 `tests/fixtures/proxy_bridge/`。

Responses JSON、SSE、WebSocket 和 WS→HTTP fallback 共享下游提交边界。`response.created` 等 lifecycle 事件不会提交响应、不会记录首 token，也不会延长 `STREAM_FIRST_BYTE_TIMEOUT_MS`；首个业务或终态事件之后才切换到 `STREAM_IDLE_TIMEOUT_MS`。Provider-origin 失败仅可在提交前 failover，client validation 失败原样返回，`response.incomplete` 按有效部分终态处理。

事故回滚只设置 `CC_SWITCH_PROXY_SEMANTIC_GUARD_ENABLED=0` 并重启服务；默认值为开启。回滚会关闭普通 Responses 的语义分类/提交门禁，但不会关闭 HMAC reasoning envelope 验证。包含 `image_generation` tool 的 Responses 图片传输仍强制执行最小 lifecycle/terminal 检查：它已经用心跳提交 wire `200`，因此必须继续把 `response.failed`、`response.incomplete` 和客户端取消写成真实终态，不能由该事故开关回滚。观察 `/metrics`：

- `cc_switch_proxy_semantic_guard_total{surface,observation}`：`lifecycle`、`business`、`success_terminal`、`incomplete_terminal`、`client_failure`、`provider_failure`、`protocol_error`。
- `cc_switch_reasoning_bridge_total{direction,outcome}`：reasoning envelope encode/decode 成功、过大、MAC 或 envelope 校验失败。

真实 Claude/OpenAI OAuth、ChatGPT upstream、Router callback、Market 和 Share grant 仍按 `docs/real-acceptance-runbook.md` 提供输入后执行；离线 fixture、mock 和 readiness 不能标记这些项目真实通过。

## 数据目录

配置目录包含：

- `server.json`
- `providers.json`
- `accounts.json`
- `accounts.key`
- `shares.json`
- `usage/`（manifest、最近 32 天请求明细、按日 journal 和长期 rollup）
- `tunnels.json`

这些文件使用原子写入方式保存。`accounts.json` 中的账号 token 以及 S2 `providers.json` 中的 Provider credential slot 共用一个根密钥，但通过 HKDF 派生为不同用途的密钥。根密钥默认保存在 `accounts.key`；也可以用 `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY` 提供 32 字节 base64 密钥，环境变量优先于文件。备份时直接备份整个 config dir，不能只备份 JSON 而漏掉匹配的 `accounts.key` 或部署环境密钥。

### Provider S1 → S2

新安装首次提交 Provider 时直接使用 S2。已存在的未迁移 `providers.json` 保持 S1；启动时只建立内存兼容视图，不会静默改盘。先执行只读预检：

```bash
sudo -u cc-switch-server cc-switch-server \
  --config-dir /var/lib/cc-switch-server \
  config migrate-provider-store
```

确认 JSON 报告中 `sourceFormat=s1`、`canApply=true`、`blockedCount=0`、`runtimePlanParity=true`，然后停止服务执行写操作：

```bash
sudo systemctl stop cc-switch-server
sudo -u cc-switch-server cc-switch-server --config-dir /var/lib/cc-switch-server \
  config migrate-provider-store --apply
sudo systemctl start cc-switch-server
```

写操作会获取数据目录进程锁；服务仍运行时必须失败。切换后的 S1 快照位于 `provider-migrations/s1-to-s2/`。回滚和显式清理同样必须停服：

```bash
sudo -u cc-switch-server cc-switch-server --config-dir /var/lib/cc-switch-server \
  config migrate-provider-store --rollback
sudo -u cc-switch-server cc-switch-server --config-dir /var/lib/cc-switch-server \
  config migrate-provider-store --cleanup-snapshot
```

在至少两个稳定 bridge release 且不少于 14 天的观察窗口完成前，不清理降级快照，也不删除 S1/name/URL reader 或旧 Provider compatibility endpoint。当前门禁记录在 `assets/contract/provider-compatibility-window.json`。

S2 只降低 `providers.json` 或不含根密钥的单个快照泄露风险。Provider 列表和详情接口保持脱敏，但已登录的 Server 管理员可在供应商编辑页查看或复制单个已分类凭据。攻击者若取得管理员会话、完整数据目录、`accounts.key`、环境根密钥或 Server OS 用户权限，仍可获得 Provider 和 Account 凭据。

备份恢复：

1. `sudo systemctl stop cc-switch-server`
2. `sudo tar czf cc-switch-server-config.tgz -C /var/lib cc-switch-server`
3. 恢复时解压到同一路径并确认权限属于服务用户。
4. `sudo systemctl start cc-switch-server`
5. 登录 Web 或调用 `/api/router/diagnostics` 检查 router/share/tunnel 状态。
