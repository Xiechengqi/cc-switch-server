# 一次性 Share Market 试用（Contract v9）

状态：权威实现说明；线格式以 Router 的 `PROTOCOL.md` 为准。真实部署／支付联调须单独验收。

Server 只处理绑定 Router 下发的 managed entitlement，不计算价格、余额或平台默认试用配置。Router 使用 `marketTrialV1` 显式协商 v9；普通未协商 descriptor 保持 v7，accountReset v8 能力独立。

## 预算与阶段

`marketEntitlements` 以 entitlement ID 索引：scope／email、`phase=trial|paid`、active、原始 trialTokenAllowance、本次 trialTokenLimit、trialTokensUsed／usageRevision、appliedAtMs、paidStartedAtMs、usageUncertain。scope 跨相同用户同一 Share 的租约保留。Token 缺省表示时间试用；不允许用普通 tokenLimit=0 表达“不限”。普通周期 quota 仍独立生效。

同 entitlement 不能改用户或 scope、不能 Paid→Trial、不能重新给试用预算。同 scope 不能绑定其他邮箱。revoke 只停授权，不删除累计计数或未完成请求；后续旧请求的 Tokens 仍能结束同 scope 的新 trial。

`state.begin_share_quota_usage` 在任何上游工作前持久化 request ID、entitlement 和当时 phase。`commit_usage_append` 先持久化累计 Token 修订及阶段 quota，再写 Usage journal；失败不得重新开放免费预算。重试同 revision 必须相同 payload，较旧 revision 不再重复计数。试用→正式时重新投影当前 entitlement 的正式请求 quota，保留 trial 累计水位，迟到 trial 响应不能扣正式额度。

流式中间进度即使已有部分 Token，也不是最终回执；admission 保持未完成，收到最终结果后才累计。若在最终结果前重启，仍按未知试用用量处理，不把中间进度误判为修订冲突。

## 崩溃与存储

`market_trials::recover` 只在真实进程启动／备份恢复调用，不在普通 snapshot load 调用。持久化 admission 无最终用量时，先从权威 Usage 恢复；不能证明完整则 `usageUncertain=true`，同 scope 拒绝新请求，通知 Router 撤权清理。备份恢复不能证明备份之后的消费，所有活动的版本化 entitlement 均需安全隔离。存储写入失败设置 degraded 标记并阻止后续市场请求，避免在旧水位继续服务。

上述免费预算隔离针对 trial 阶段发起的请求，包括转正式后才返回的 trial 结果。普通 paid 请求的取消／中断沿用既有付费额度统计规则，不因一次取消触发试用回收。

精确 request receipts 保留 35 天，覆盖 Usage 的 32 天明细／修订窗口；之后最终结果压缩进分 entitlement／phase 的 lifetime 总计，永久 trial 累计不减，普通最长 30 天／自然月窗口不受影响。pending receipt 不删除。最近／未完成 receipts 超过每 Share 1,000,000 条时拒绝新的 market admission，不能丢历史腾位置。额度索引与 receipts 随 Share 持久化，存储锁按 state 域方法统一获取；与 workspace rebind 共用事务门，先 workspace、后 usage、后 shares，避免反向锁等待。

## Router 转付费确认

Router 先预留资金，再下发 paid phase；Server 原子应用 policy 和 paidStartedAtMs 并返回、同步确认。Router 确认 phase、有效 policy 和时间 fence 后才捕获首期价款。拒绝、未知回执、用量不完整都走安全撤权；没有“先扣款再猜测授权成功”。未确认清理前不能复用车位或释放对应预留款。

## 本地验收

单元测试覆盖跨 quota 周期不重置、阶段切换与迟到用量隔离、跨 entitlement scope 消耗、重启未知用量隔离、receipt 压缩与 lifetime 保留，以及动态用量不改变静态 descriptor 指纹。需与 Router 的冻结报价、保留资金、仅一次扣款及安全释放测试共同执行。真实上游调用、生产部署与真实付款不属于本地测试结论。
