# Client 身份恢复与市场离线下架

状态：**权威**。2026-11-27。线格式以 Router `PROTOCOL.md` 为准。

## 身份恢复

Router 的普通离线 GC 保留 `client_identities` 与域名预留；原公钥签名重新注册返回原 installationId。运行记录重建时签发新的控制凭据，Server 原子保存注册响应，不恢复过期会话。Provider、Account、Share、加密根密钥不属于该恢复写入范围。

同一 Router、公钥不变且已存在 installationId 时，Server 不接受未经证明的新 ID。异常返回 `client_identity_recovery_required`，保留原 ID 与密钥。历史孤立域名经 Router 管理员显式核验修复后，注册响应可追加 `previousInstallationIds`；只有当前本地 ID 在该列表内才接受恢复到原 ID，并清除旧 ID 的 claim intent。

原机已销毁的市场挂售项由 Owner 在 Router 下架，不要求恢复 Server。相同 IP 上运行的另一 Router / 公钥实例不能代替旧身份提交回执。

## 恢复依赖

注册及 Owner → Client 控制隧道 → 市场协调及 Share 恢复。Share actor 可以先排队，但控制隧道尚未就绪时停在 `waiting_client`，不请求 Router、不累计租约失败。就绪必须同时匹配当前 Router、installationId、当前活跃 actor 和连接状态；磁盘残留 `connected` 状态不能作为证据。

后台元数据同步在控制隧道就绪前保留待同步状态。域名身份冲突使 Client actor 进入 `blocked`，停止无意义的自动重试；注册修复或显式重连可重新启动。

## 市场下架

Router 的市场下架和 Server 本机解除挂售锁是两个状态。Owner 下架立即停止新订单，当前租约继续，退款、撤权、清理和归还仍按原状态机执行。离线意图等待重连；已有租约命令先完成，随后执行更高序号的 `unlist`。清理未确认不允许重新出租，不能通过 Owner 下架伪造清理回执。

## 验证边界

补充已知身份不能静默替换、历史别名恢复、磁盘状态不等于活跃 actor 的回归用例。遵守用户限制：不部署、不重启远程服务、不执行运行类或 UI 测试；仅执行编译、格式、类型和纯源码审计，动态验收不标记为通过。
