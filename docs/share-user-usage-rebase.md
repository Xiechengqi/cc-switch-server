# Share 用户周期用量重基线

更新：2026-08-19

## 目的

Provider 官方可能在任意时间把账户额度重置为 0。管理员需要把某个 Share 用户的周期起点改到官方重置时间，并把“已消耗 Token”改成该时间以来的实际消耗；保存后新的请求必须在这个数值上继续累加。

## 权威语义

- Usage 日志是追加式事实来源，不会因为 UI 编辑而被覆盖或删除。
- `ShareUserGrant.usage` 是当前 policy 的派生快照。
- `ShareUserGrant.usageRebase` 是 Server 持久化的重基线记录，包含周期、anchor、窗口、目标值、保存时观测水位和 Usage journal watermark。
- `userUsageEdits` 只接受显式 `set` 或 `clear` 操作；浏览器提交的 `usage` 快照永远不可信。
- `targetTokens` 表示保存瞬间希望得到的最终有效用量。若目标小于 Server 观测值，保存返回冲突，避免误操作把配额降低。
- 有效用量计算为：

  ```text
  targetTokens + max(observedNow - observedTokensAtRebase, 0)
  ```

  当周期或窗口改变时，旧重基线失效并清除；没有重基线时直接使用观测值。

## 周期与时间

`sevenDays` 和 `thirtyDays` 使用现有 UTC 固定相位窗口。`tokenPeriodAnchorAtMs` 必须是分钟精度、不能在未来，并且可以是过去的时间点。保存时 Server 重新计算当前窗口起止时间；UI 同时显示窗口预览。Lifetime、日、自然周、自然月不使用 anchor。

## 保存协议

Provider Share 和 Provider Bundle Share 的保存 payload 可带：

```json
{
  "userUsageEdits": {
    "user@example.com": {
      "action": "set",
      "targetTokens": 12345,
      "expectedGrantRevision": 8,
      "period": "sevenDays",
      "anchorAtMs": 1782907200000,
      "source": "providerReset"
    }
  }
}
```

清除重基线使用 `{"action":"clear"}`。`expectedGrantRevision` 防止管理员在旧页面上覆盖并发修改；Router Share Market 管理的 grant 始终只读，不能通过该字段修改。

`usageRebase` 纳入 Share descriptor 的静态 fingerprint，但高频变化的 `usage` 仍被排除。因此手工重基线会触发一次新的 descriptor generation，普通请求计数不会造成同步风暴。

## UI 规则

- 用户限制编辑中显示当前周期有效用量、重基线目标、观测值和 UTC 窗口。
- 输入 `0` 是有效的显式目标；留空不会把空值转换成 0。
- “清除重基线”只删除手工基线，Usage 历史仍保留。
- 批量编辑不提供“已消耗 Token”批量复制，避免把一个用户的消费量误套到其他用户。
- Router Share Market grant 标记为只读。

## 一致性与限制

- Share 总 Token 限额和 Provider 官方 quota block 与用户重基线分离；重基线不会恢复已耗尽的 Share 总额，也不会清除 Provider quota block。
- 请求完成后，Router 只消费 Server descriptor 中的有效 grant usage；Server 是 grant quota 权威。
- Usage journal watermark 目前用于审计和并发诊断，增量仍采用窗口快照差值；Usage rollup 与迟到日志由现有 `share_user_quota_usage` 统一计算。
- 真实 Router、Provider reset 和 Share Market entitlement 的端到端验收仍需外部输入；本地测试不宣称真实外部通过。

## 验收重点

1. 保存 `targetTokens` 后立即发起请求，用户用量变为目标加新请求 Token。
2. 修改过去的 7/30 天 anchor 后，窗口只统计新窗口内日志。
3. 目标低于观测值、旧 grant revision、Router Market grant 编辑分别返回稳定冲突码。
4. 清除基线后，窗口用量回到 Usage 历史观测值。
5. descriptor generation 在 rebase 改变时递增，单纯 request counter 变化不递增。
