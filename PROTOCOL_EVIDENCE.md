# Protocol evidence policy

`cc-switch-server` 的 Provider 产品范围、身份和运行时行为由本仓库维护，当前权威来源是：

- `assets/contract/server-provider-requirements.json`：必须覆盖的 ProviderType 与 App Surface；
- `assets/contract/provider-registry.json`：Family、Profile、Driver、凭据与模型策略；
- `assets/contract/provider-legacy-compatibility.json`：兼容窗口内只读的 S1/旧 Web fixture；
- `assets/contract/*-protocol.json`、本仓库测试及厂商公开协议：具体 wire 行为证据。

外部仓库可以在一次明确的协议研究中作为只读差异证据，但不是实现同步源，也不能成为构建、测试、发布或运行时输入。吸收证据时必须在本仓库形成最小合同、独立实现与回归测试；证据不足的能力保持 `live_pending` 或 fail closed。

历史上从其他开源项目改编的代码与界面工作记录在 `SOURCE_PROVENANCE.json`，完整归属与许可证见 `THIRD_PARTY_NOTICES.md`。这些记录用于合规和溯源，不赋予外部仓库当前产品权威性，也不要求 CI checkout 外部源码。

Provider 合同变更至少运行：

```bash
node scripts/audit/audit-server-provider-contract.mjs
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
```
