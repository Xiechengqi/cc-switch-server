# 供应商展示身份与跨产品图标

> 状态：权威。线格式以 Router `PROTOCOL.md` 的 `providerIdentityV1` 为准。

## 权威与边界

Rust Provider Registry 决定真实 Family/Profile；`assets/contract/provider-presentation.json`
为每个 Family 声明单一展示分类和默认图标，精确覆盖当前全部 37 个 Family。名称、显式
图标及颜色来自保存的 Provider 展示配置。App API、上游协议、兼容 ProviderType、模型、
URL 和自选 Logo 都不决定分类。例如 MiniMax 同时提供 Claude/Codex API，仍只属于
MiniMax；自定义供应商选择 OpenAI Logo，仍属于自定义。

`src/domain/providers/presentation.rs` 输出身份，附在 descriptor 的
`appProviders.*[].providerIdentity`。Router 按真实绑定聚合为一个 Share 级身份，只有
Bundle ID 相同且展示内容一致才合并多绑定。不能证明同一 Bundle 的历史多供应商分享
显示为 `legacy_mixed`；缺失、未知或矛盾信息显示为 `unknown`。不猜品牌、不取第一个
绑定、不改绑或停用分享。

## 资产发布

现有本地 `web-src/src/icons/extracted` 的 72 个图标及 metadata 是图标源。生成物包含
资产内容、渲染方式、默认色、逐资产 SHA-256 和内容寻址 `catalogVersion`。彩色/图片
资源不转单色 mask；inline SVG 保留原始颜色及 `currentColor` 语义。

```bash
node scripts/generate-provider-presentation.mjs
node scripts/generate-provider-presentation.mjs --check
# 仅联合发布时明确导出冻结数据，不是构建/测试/运行时依赖。
node scripts/generate-provider-presentation.mjs --export-router /path/to/cc-switch-router
```

Server 使用本地源资产及同一生成契约，Router 独立消费已提交的冻结包；各自构建不读取
另一个工作目录。不复制 UI 组件，不按名称另建猜测表。显式 icon 覆盖默认值；未知 ID
回退首字母，不接受远程图片 URL、任意 SVG 或 HTML。颜色优先保存值，其次图标
metadata。新增 Family/图标时同步重新生成 catalog 和跨仓 fixtures。

## 协商与同步

先升级 Router reader，再升级 Server writer。Router heartbeat 声明 `providerIdentityV1`；
Server 仅在绑定 Router URL/installation 且能力未超过 300 秒时发送字段。descriptor、
子域申领、隧道租约及 runtime 出口均做能力投影，旧 Router 不收到新嵌套字段。启动和
能力变化后自动补发已有分享；失败复用 descriptor generation/fingerprint/ACK 重试，
不改变管理员 `configRevision`。

身份是静态展示投影：名称/Logo/颜色变化推进 descriptor ACK，但不进入 Provider 执行
或健康指纹，不重绑凭据、不清健康历史、不旋转隧道。Router runtime 刷新只是观测，
不能覆盖已确认身份；按 app/providerId/Bundle ID 精确匹配保留静态内容。旧 writer
省略字段时不抹掉已知身份；真正更换绑定时不得串用旧身份。

## 市场与租约隔离

公开 DTO 仅含 `schemaVersion/familyId/category/displayName/icon/iconColor/catalogVersion`，
不含 Bundle ID、Provider ID、账号或凭据源。Client 卡片、抽屉、模型入口和市场统一消费
Share 级身份；App 图标独立表示接口类型，每个分享仅归入一个展示分类。

历史 `providerFamily/providerFamilies` 保持兼容读取，尤其租约不可变
`service_snapshot_json` 的 `providerFamily/providerType/model` 语义不变。展示分类只读
`providerIdentity.category`，不能因分类修复触发暂停、退款、撤权、计费或合同事件。

## 验证

生成 fixtures 覆盖每个 Family 的全部 Surface。两仓 Rust 验证身份产出与消费；回归覆盖
多 API 单 Logo/单分类、颜色覆盖、彩色/光栅资源、安全回退、旧 reader 字段省略、延迟
runtime、持久化往返和本地活跃租约快照不变。真实部署联调与浏览器人工验收另行执行，
不能把离线测试标为真实通过。见 [UI 清单](../acceptance/manual-ui-checklist.md)。
