# 普通授权 Token / USD 即时切换

> 状态：权威实现规范。2026-11-11。本地仅编译测试目标、类型与静态审计；产品测试由 CI 执行，真实 Provider / App 验收保持 `live_pending`。未部署。

普通授权的单位、USD 周期或 anchor 改动须先获取当前窗口签名预览，再与 Share 设置原子保存。历史真实消费、人工调整和在途风险分别承接，不创建新窗口或清零。只改同一 USD 窗口的额度仍直接生效；Share Market 托管合同沿用原合同规则，不使用本协议。

例如 100M Token 已使用 30M，冻结执行价目证明其为 $42，切为 $100 后保存立即显示已用 $42、剩余 $58。中途切回 Token 的生成持续保存金额事实，再切 USD 时计入这段消费。目标额度低于已用可保存；剩余显示 0，之后新生成按最新策略拒绝。

## 事实与历史准备

SQLite schema 6 的 `quota_metering_facts` 独立于报表保留期，按 request / attempt 保存不含正文的模型、tier、Token 分类、资源量、估算上界、结果、修订与账号归属。Token 请求先提交事实；有价时冻结可选金额，缺价不阻断 Token 请求。USD 准入仍须有价并保留风险。

历史准备是显式写接口，预览本身只读。每次最多 500 条日志或事实，持久保存游标、阶段与迁移价目修订，超时或重启可继续。已经入账的请求不重价；首次补价的事实冻结本轮迁移价目。完成后可重新准备，补获迟到日志或新事实。索引限定 Share、规范化 email、request 游标；账号金额按 source / epoch / bucket 及周期边界隔离。

历史完整性同时核对独立 Token 汇总、水位、请求数、每个重试 attempt、未定价事实与金额缺口。只剩汇总但缺模型 / Token 分类 / 资源等证据时拒绝切 USD，不能将缺失历史当成 0。升级时固定旧日志的 Router / installation 身份，排除市场身份，禁止后续配置切换把旧消费重标到另一主体。账号缺归属同样阻断。

冻结 tariff、alias、estimate、资源计划与 hash 随 attempt 保留；压缩目录或日志不会影响迟到修订结算。异步 task 允许绑定未定价 Token fact，轮询验证 task、父 request、Share 与用户，终态事实独立于可选金额结算保存；完成后的迟到失败不能清除消费。

## 预览、校准与并发

`quotaUnitConversionV1` 要求 Router reader ≥12，能力缓存绑定 Router URL / installation，300 秒有效。v12 grant 新增 Server-owned `quotaConversionId`；转换后的 Token grant 保留 `valueMetering`，计量起点只能向历史扩展，不能前移抹去消费。

预览凭据为 Server 身份密钥的 HMAC，最长 15 分钟且不跨当前窗口结束。绑定 scope、配置 / grant revision、源 / 目标 policy、窗口、origin、账号周期 revision、人工调整身份、价目依据和有效期。消费水位不作为过期条件：预览后新增消费在保存时重新读入。伪造、到期、修改策略 / 价格 / 周期等返回冲突并要求重预览。

真实消费不可删除。源单位存在人工调整，且无可复用跨单位映射时，必须输入目标已用及原因；资源金额或保守金额无法天然对应 Token 时也必须校准。目标不得低于预览真实观测量。偏移按预览观测值计算，之后消费照常叠加：$42 预览校准到 $50，保存前再消费 $14，最终为 $64。正反向映射保存审计身份，往返切换只复用偏移一次。两端单用户 / 批量草稿保留独立单位输入，预览展示已用、在途、剩余及证据状态。

新生成为短事务重新读取 enabled、撤销、App、到期和最新额度。锁序 config → usage → shares 提交 gate；发网和流生命周期均不持 gate。策略变更、转换和准入通过同一提交 gate 串行。已有生成继续用其原 request / attempt、窗口、模型、tier 和冻结价；切换不取消流，也不重发请求。

提交重新核对历史和窗口，并将可证明的在途风险提升到目标单位；不能证明上界时拒绝切换。转换后的 Token 准入也考虑在途 Token，按 invocation 合并重试上界，避免借换单位释放预占。USD → Token 不依赖新模型目录有价，仍要求历史 Token、来源状态与在途证据可用。

## 原子性、恢复与协议

保存沿用 `editContext.operationId` 和完整 patch fingerprint。一个 SQLite IMMEDIATE 事务内写策略、binding / Token baseline、金额调整与映射、转换回执和永久 settings receipt；所有用户同成同败，commit 后才发布内存。4096 条内存 settings receipt 淘汰后仍可从永久回执幂等恢复。相同 operation 不同 payload 返回冲突。

Router 迁移 112 保存 Server 转换回执镜像；同步控制回复和异步 edit ACK 均携带 `quotaConversions` 与 `valueQuotaStatuses`，旧空字段省略以保留旧签名。Router 事务内保存 descriptor、完整回执批次、当前窗口投影及 applied edit。旧策略的延迟回执只归档，不覆盖新窗口。owner 可读取 edit 回执恢复响应丢失；Server 回执查询同时限定当前 Router / installation / Share。

控制面接口和字段以相邻 Router 仓库 `PROTOCOL.md §13.5` 为权威。Server Web 命令为 `prepare_share_quota_history`、`get_share_quota_conversion`；签名控制面为 `/_ctl/prepare_quota_history`、`/_ctl/quota_conversion_receipt`。缺协议能力、陈旧凭据、缺历史 / 在途证据、恢复 fence、批次冲突均为 `QUOTA_CONVERSION_*` 类型化 409，保留原策略与草稿。

已有 `pendingValueQuota` 不因升级提前激活；后台继续按原 effectiveFrom 激活。用户主动替换旧排期时走本协议。能力丢失时隔离带转换 marker 的 Share，同批无关 Share 可继续同步，禁止向旧 reader 静默降级。

## 验证与发布

CI 回归覆盖 $42 承接、往返期间消费、低额度立即耗尽、预览后新增消费与校准、在途预占、账号隔离、缺失事实、批次回滚、永久幂等、冻结价目压缩、分页恢复、十万条索引路径、未定价异步任务和凭据篡改。此清单是已编写的回归范围，不代表本地已执行通过。

先发布 Router，再发布 Server；不自动修改生产授权、历史金额或存储 authority。schema 6 / v12 一旦使用，回退须保留可读取全部事实与回执的兼容版本或受控恢复备份，不能删除账本或清空 origin 规避冲突。
