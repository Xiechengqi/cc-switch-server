# Kiro / Amazon Q 图片处理

状态：权威。本页描述 Server 的本地图片策略；真实厂商接受性仍为 `live_pending`。协议来源和本地策略分别记录在 `assets/contract/kiro-image-budget.json`，历史 wire fixture 不据此改写。

Kiro OAuth 和 Amazon Q OAuth 使用各自固定账号、产品、凭据、区域、目录及端点。图片处理器共用不会授予跨产品权限。Claude Messages、Chat Completions 和 Responses 的图片转换最终都检查 `userInputMessage.images[].source.bytes`：每图最多 **400000 个 ASCII base64 字符，包含 padding**。这是本地保守预算，尚无证据证明为官方限制。

每请求最多 20 图，每图解码后的输入最多 8 MiB，历史总输出上限为 20 MiB；解码长边最多 16384，像素最多 64 Mi。静态图需要转换时先应用 EXIF 方向，限制长边 1568，再依次降低 JPEG 质量和尺寸。每图最多 24 次编码，每请求最多 64 次编码和 256 Mi 像素工作量；最终字段超限或工作预算耗尽明确拒绝。GIF 在字段预算内保留原字节及动画，超限拒绝，不转成静态图。

生产图片转换进入最多两个任务的 blocking 执行器，无等待队列；满载返回本地 503。超时返回本地 504，取消在处理检查点生效。正在执行的解码、缩放、编码不会因异步调用者退出而立即停止，因此执行许可和内存预约由真实 worker 保留到退出；已完成但尚未接收的结果也保留预约。每个已处理字段在整个请求 body 接管预约前持续计费。

图片处理后重新核对账号实例、身份/凭据代次、产品/区域身份和 Provider revision/runtime fingerprint；绑定发生变化则拒绝使用结果。这些本地错误不授权刷新、换账号、换区域、换端点或重放生成。现有严格 EventStream CRC、终态、错误与使用量合同继续适用。

推荐源码中的 `runtime.{region}.kiro.dev` 尚缺独立的产品、区域和凭据接受性证明，保持 KI-02 研究门禁；本页不启用新域名。
