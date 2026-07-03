# Provider Fixture 目录

本目录用于保存从 `/data/projects/cc-switch` 只读导出的 Claude/Codex/Gemini provider fixture。

导出命令：

```bash
node scripts/export-current-cc-switch-fixtures.mjs
```

约束：

- 脚本只能读取 `/data/projects/cc-switch`，不能修改上游项目。
- `structures.json` 保存脱敏结构覆盖情况，包括 `settingsConfig/meta/models/modelMapping/testConfig/authBinding/codex config/gemini config`。
- fixture 用于 adapter contract test、provider type 分类回归、usage parser snapshot。
- OAuth/账号型 provider 没有真实凭据时，只能保存脱敏配置结构和协议样例，不能标记真实登录能力完成。
