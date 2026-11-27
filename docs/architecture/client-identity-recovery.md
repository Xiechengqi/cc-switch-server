# Client 身份恢复与市场离线下架

状态：**权威**。2026-11-27。线格式以 Router `PROTOCOL.md` 为准。

## 身份恢复

Router 的普通离线 GC 保留 `client_identities` 与域名预留；原公钥签名重新注册返回原 installationId。运行记录重建时签发新的控制凭据，Server 原子保存注册响应，不恢复过期会话。Provider、Account、Share、加密根密钥不属于该恢复写入范围。

同一 Router、公钥不变且已存在 installationId 时，Server 不接受未经证明的新 ID。异常返回 `client_identity_recovery_required`，保留原 ID 与密钥。历史孤立域名经 Router 管理员显式核验修复后，注册响应可追加 `previousInstallationIds`；只有当前本地 ID 在该列表内才接受恢复到原 ID，并清除旧 ID 的 claim intent。

原机已销毁的市场挂售项由 Owner 在 Router 下架，不要求恢复 Server。相同 IP 上运行的另一 Router / 公钥实例不能代替旧身份提交回执。

## 恢复依赖

注册及 Owner → Client 控制隧道 → 市场协调及 Share 恢复。Share actor 可以先排队，但控制隧道尚未就绪时停在 `waiting_client`，不请求 Router、不累计租约失败。就绪必须同时匹配当前 Router、installationId、当前活跃 actor 和连接状态；磁盘残留 `connected` 状态不能作为证据。

后台元数据同步在控制隧道就绪前保留待同步状态。域名身份冲突使 Client actor 进入 `blocked`，停止无意义的自动重试；注册修复或显式重连可重新启动。

模型健康探测结果继续保存在本地。对 Router 的刷新通知仅加入按 Share ID 合并的待同步集合，不绕过 Client 控制隧道依赖直接发送。发送还要求对应 Share 的当前 actor、完整 spec（Router、身份、Share、域名、本地端口）、活跃 transport 和当前描述 ACK 均有效；无就绪证据时静默保留待同步项。最多四路并发、单次网络等待十秒；真实失败保留重试，发送期间的新更新也不会被清掉。进程重启时按启用且 active 的本地 Share 重建刷新项，不恢复历史删除 Share。

Client 就绪同样匹配完整 spec 和 `transportState=active`，不能仅凭原 Router/ID 前缀、磁盘残留 connected 或尚未退出的旧域名 actor 放行。

## 本地初始化与远程就绪分离

`server.json.initializedAtMs` 记录已完成的本地初始化；旧配置含既有密码哈希和 Owner 时在加载及下一次保存中兼容记为 `0`，新初始化记录实际时间。此状态不因注册失败、域名冲突、网络断开或修复 transport 配置回退。配置替换、域内修改和持久存储 reload 均保留已知初始化标记；租户工作区继续由自身 grant 判定初始化，保存时不覆盖 installation 身份。

`is_initialized()`（兼容方法 `is_setup_complete()`）控制初始化入口与本地登录。`is_local_setup_complete()` 表示恢复 transport 所需配置齐全；`is_router_client_ready()` 仅表示配置中的已知 claim 状态，不能证明当前在线。动态就绪仍依赖活跃 actor。`/api/setup/status` 返回 `initialized`、`needsSetup` 和 `routerConfigurationReady`，Router claim 失败时不再提供匿名 bootstrap/complete-setup 能力，也不再阻止既有本地密码登录。

认证继续执行在租权限隔离：Client 被租用后，Owner 的本地密码、API Token 和既有会话都不能管理当前租户工作区，Web terminal 继续关闭。初始化修复没有增加 Owner 绕过租约的路径；密码修改入口和修改后强制重新登录保持原契约。

启动日志仅在真正未初始化时打印 init/bootstrap 示例。本地已初始化但缺少 Router 配置时明确提示修复配置；远程注册成功不等于 Client 隧道已经就绪。持久 tunnel 状态增加 `blockedReasonCode`，保留具体错误；即使没有 tunnel URL，Web 状态仍返回原因及 `recoveryRequired`。设置页显示恢复提示和显式“重新检查”，不把身份冲突显示为普通重连或建议清空数据。

Router 状态和诊断接口的 `connectivity` 区分 `setup_required`、`configuration_required`、`registering`、`claim_conflict`、`identity_recovery_required`、`blocked`、`reconnecting`、`heartbeat_delayed`、`stopped` 与 `online`，并返回本地初始化、配置齐全、已注册、控制隧道在线以及恢复原因等独立字段。历史磁盘状态可以解释错误，不能让 `clientTunnelOnline` 变成 true。

## 历史恢复的应用边界

2026-11-27 的 sgptokenswitch 实例在持久身份迁移之前就丢失了原 Router runtime：当前本地 ID `f08cb9e8-b1e8-447c-9e0d-aff0f19da839`，但 `r0pu9iwq81` 仍属于原 ID `d5920f1b-c59f-4dc4-bc2c-f432037edc16`。两份受信 Router 旧备份的完整公钥与当前 Server 一致。直接运行 Server 无法替代 Router 端的显式历史修复；重新初始化同样不能解决域名归属冲突。

Router 原恢复实现存在公钥唯一索引下的写入顺序 bug，已改为共享预检计划、必需 `expectedPlanHash`、先删除临时 runtime 再完整重建原 runtime，并同事务提交锚点、别名和审计。只读准备工具、备份哈希、现场参数和后续步骤见 Router `docs/client-identity-recovery-and-offline-unlisting.md`。该工具不访问 Server SQLite，不导出私钥或供应商凭据。远程 Python SQLite 版本无法解析较新 Server schema 时，不得据此判断数据库损坏或清空存储。

本次未执行生产身份应用，也未部署或启动 Server。后续 Router 显式恢复成功后，正常注册或设置页“重新检查”可接受 `previousInstallationIds` 证明并恢复原域名、有效 Share 和本地账号；不需要 init、删除配置目录或重建供应商账号。

已启动进程在接收 Router 已确认的临时 ID 别名并持久化原 ID 后，唤醒 Share 协调器。它先建立使用原 ID 的 Client 控制隧道。若磁盘上留有连接记录但对应任务已退出，协调器重建连接；原 ID 自身仍有域名冲突时保留阻塞状态。控制隧道就绪后，协调器尝试同步描述，并按本机启用清单核对每条 Share 的连接配置。匹配的活动任务可复用，临时 ID 的任务由新一代替换。模型健康通知等待描述确认与当前传输，重启后由本机有效 Share 清单重建待办。

若完整同步因单条 Share 或清理请求失败，已联网且有健康更新待发的 Share 会单独用当前身份提交描述。Router 接受该 Share 的描述、当前隧道仍然就绪时即可上报健康；被拒绝的 Share 留待下轮重试。此前临时 ID 下匹配的描述版本不足以放行这次上报。

Server 按协商能力使用原 ID 获取当前适用的文本或资源费率，重新签名确认，并在费率生效后报告激活。临时 ID 的回执随该身份退休。本机继续使用原 ID 的额度账本、已有 Provider、账号和密钥。

## 市场下架

Router 的市场下架和 Server 本机解除挂售锁是两个状态。Owner 下架立即停止新订单，当前租约继续，退款、撤权、清理和归还仍按原状态机执行。离线意图等待重连；已有租约命令先完成，随后执行更高序号的 `unlist`。清理未确认不允许重新出租，不能通过 Owner 下架伪造清理回执。

## 验证边界

补充已知身份不能静默替换、历史别名恢复、旧 spec/磁盘状态不能放行、claim 失败仍可本地登录且 bootstrap 关闭、持久初始化不丢失、无 URL 时恢复原因保留、blocked actor 停止重试、健康通知依赖 ACK 与活跃隧道、并发排队更新不丢失等回归用例。遵守用户限制：不部署、不重启远程服务、不执行运行类或 UI 测试；仅执行编译、格式、类型和纯源码审计，动态验收不标记为通过。

本轮完成整体源码 review；Server 全目标 `cargo check --locked --all-targets`、全目标 Clippy `-D warnings`、全部 Rust 格式、Web TypeScript、Provider Phase 0 生成/检查、coverage/UI matrix、Web runtime/i18n/产品边界及文档索引审计通过。新增回归用例随全目标编译通过并加入 CI 过滤，当前没有执行。Router 侧检查及本次范围外的格式差异见其恢复文档；远程身份仍待修正后的 Router 可用后预检/应用，本次没有修改生产数据。
