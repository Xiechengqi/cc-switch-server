# Share Market 最终撤权与消费回执

状态：权威，Server 侧实现。线格式以 Router `PROTOCOL.md §7.1` 为准，业务规则在 Router `docs/share-market-immediate-termination.md`。Server **不**计算租金返还、退租补偿、佣金或钱包余额。

## 协商与存储

Router heartbeat 独立声明 `shareTerminationV1`。Server 缓存须绑定 Router URL 与 installation，300 秒失效；不能根据 reader v10 或试用能力推断它存在。

SQLite schema v4 增加 `market_settlement_requests`、`market_settlement_receipts`、`market_settlement_ack_outbox` 和真实 `market_settlement_since_ms`。只有 SQLite authority **committed** 时，权威 Share 读取和普通 Share 同步才发布 `terminationArchiveSinceMs`；旧 Router 必须省略该字段。该字段参与静态指纹，升级后的普通 Share 同样会重新同步。保留旧迁移 checksum，v4 的 SQL 也有独立 checksum；未知更高版本拒绝，不允许旧 binary 降级写入。

请求归档独立于 Share receipt 的 35 天压缩、`usage_records` 的展示保留期及用户历史记录。Share 提交／引用图提交都会在同一 SQLite 事务中归档请求身份、entitlement、邮箱、trial／paid、开始时间、Token 数和修订。更高修订先重开 pending 状态；之后仅相同修订且身份和数量一致的 Usage 事务可以确认最终状态。pending、interrupted、parse error 和成功但缺 usage 都不能证明最终零消费。

升级或崩溃后可从已经持久化的、身份和修订完全匹配的 Usage row 恢复终态，不能仅凭 `revision > 0` 认定已完成，不能从不存在的展示日志推导零值。

## 原 entitlement 撤权

1. 仅 Router managed revoke 可带 `termination` fence。限制其身份、时间范围和动作类型；普通设置入口仍拒绝 managed grant。
2. 先通过既有 Share 域方法原子撤销原 entitlement，落库 pending ACK 标记。不得按邮箱删除后来的新授权，不影响其他拼车用户。
3. final receipt 在 SQLite IMMEDIATE 事务中读取原 Share，并确认原授权不再活跃，随后排空该 entitlement 所有在途请求。原试用请求和截止附近竞态也要确认最终状态，但租金消费只包含截止前的 paid 请求。
4. Token 车位取真实归档 Token；美元等值车位读取执行价目 ledger 的全部终态尝试和已结算 picos。不使用 UI 估价、当前定价或字段缺失。成功但无执行账本不能假定零；只有最终失败且零消费的凭据可以证明未执行。
5. 按原额度窗口聚合消费，同时给出本付费周期前的已付费累计消费。累计额度的窗口使用本付费周期边界，其他周期使用 UTC 自然或原锚定边界。
6. 不可变回执绑定原 operation／termination／entitlement／Share／邮箱及计费区间；相同操作重放返回原回执，包括原撤权时间，不重新统计或重定价。

回执等待期间用 rejected ACK 的 `market_termination_pending`、`retryable=true` 表达。它不是“撤权没发生”：Server 保留已应用操作的指纹，Router 同样保留原 edit 重试，不能改变 expectedRevision 后复用 operation ID。只有 Router 接收 applied ACK 后，Server 才清理 pending 标记及操作重放缓存。多个失败重试留下的同一事项标记一起清理。

最终 applied ACK 在发出前写入独立 outbox，并绑定原 Router URL、installation、operation 和不可变回执。周期同步先有界重放最多 25 项，按最近尝试时间公平轮转，不依赖 Router 是否仍返回 pending edit；更换 Router 或 installation 不发送旧目标的 ACK。确认 Router 接收后先持久化 Share 标记清理，最后删除 outbox。网络丢失响应或任一步骤后崩溃都可重试，不会造成永久删除门禁，也不重复结算。

活跃市场授权或成功回执尚未被 Router 确认时，普通与带 revision 的 Share 删除均返回 `cc_switch_share_market_settlement_pending`，避免删除操作使最终回执永久不可达。物理数据丢失不是撤权／零消费证明，不自动放行资金。

## 故障和边界

- SQLite 尚未 committed 或归档不覆盖原周期：不发布能力／不构造可结算回执。
- 已撤权但用量不确定：访问保持撤销，继续等待，不重新授权、不推断零、不把异常转为成功。
- USD pending／unavailable 尝试：等待执行账本恢复，不能使用当前市场价格代替执行价目。
- Router ACK 网络失败或确认后本地清理前崩溃：保留原 Share、pending 标记、原操作和已准备的 ACK，重启后从 outbox 重试；回执不重复产生资金事件。
- Router 已收回执但财务失败：由 Router 独立恢复；Server 不处理资金、不替代责任裁决。
- 旧 JSON 权威实例、旧 Router 或旧付费期：不伪造历史证明。上线先 Router，再 Server；未具备能力时保留原租约取消规则。

## 验证边界

`repository::server_sqlite::market_termination::tests` 覆盖请求修订／身份冲突、试用与竞态排空、累计期初消费、回执重放与缺失 USD 执行证据；ACK outbox 回归覆盖丢失响应、持久化载荷不可变和原 Router／installation 隔离；Share 域和 state 回归覆盖操作回放与删除门禁。编译检查与静态审计不代表运行过这些回归，也不代表真实 Router 联调通过。不得将缺实际输入的真实验收标为完成。
