# Provider Fixture 目录

本目录只保留说明文档。运行时使用的脱敏结构覆盖 JSON 已迁入 `assets/contract/provider-fixtures/structures.json`。

约束：

- `assets/contract/provider-fixtures/structures.json` 是固定的 legacy Provider 兼容证据，不从外部工作树自动重生成。
- 新增结构只按 Server reader、writer 和 runtime contract 的实际需求手工补充，并在 review 中说明消费路径。
- fixture 用于 adapter contract test、provider type 分类回归、usage parser snapshot。
- OAuth/账号型 provider 没有真实凭据时，只能保存脱敏配置结构和协议样例，不能标记真实登录能力完成。
