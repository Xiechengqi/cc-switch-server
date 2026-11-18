# Share 独立扩展能力

> 权威实现规范，2026-11-17。代码实施、整体 review 和本地回归已完成；未部署、未调用真实付费上游。真实 Provider / App / 模型及生产采用保持 `live_pending`。线协议以 Router `PROTOCOL.md §13.6` 为准。

## 产品与授权

Share 设置有独立「扩展能力」，Token、USD、无限额都可管理。新 Share 默认全部关闭；新导入 Share 同样关闭，覆盖已有 Share 保留原策略。旧无 `capabilityPolicy` 的 Share 保持 legacy，须显式预览采用；升级不会自动收窄或扩张旧权限。

基础模型交互继续支持图片、PDF、音频和视频理解输入、推理、缓存、结构化输出和客户端 function/custom tools。开关控制新生成或供应商执行工具，而不是输入数据类型。目录包括图片生成/编辑、视频生成、音频输出、网页搜索/读取、文件搜索、供应商代码执行、hosted computer 与 remote MCP。最后两类当前没有可靠执行合同，不能勾选；视频只开放已实现的 Grok 路径。没有添加新的生图、生视频、TTS、embedding 或 batch 上游接口。

`domain/providers/share_operations.rs` 从编译后的 Registry runtime 及 Driver 开关派生可拦截项，不以供应商品牌或价格存在作为支持证明。现有 Grok 账号证据按身份 generation/过期时间复用，其他组合保持 unknown / live_pending。成功账号证据也不证明所有模型、流式和计费异常均已真实验收。

Share 策略显式允许操作和 App/运行时指纹。grant 缺省继承，空列表禁用，非空列表收窄；owner 调用自己的 Share 同样检查。新增绑定、账号身份轮换、模型或执行开关变化不自动继承旧许可。旧 Grok 明确授权仅迁移原同一 Grok 绑定，不扩散到其他 Provider。

## 请求执行

操作识别位于 `domain/sharing/capability_intent.rs`，只检查协议控制位置和最终模型，不扫描 prompt/schema/history。OpenAI image_generation 缺省或 auto 需要生成和编辑两许可；显式 action 只需对应许可。Gemini 图片输出加图片素材需要两许可；显式 TEXT 输出仍允许 image-capable 模型做理解。Anthropic computer/bash/text_editor 属客户端工具，不按 hosted 操作拦截。Gemini 同一对象组合多个已知工具时逐项检查全部操作；未知服务端工具类型或混入的未知控制字段在发网前拒绝。

`state::admit_share_value_request` 在价格解析之前执行集中能力检查，与配置保存共享短 gate，随后记录能力 request/attempt 证据，再进行原有 Token/USD 准入。owner、Token、USD 和真实 Share 的健康检查均不能绕过；仅内部 synthetic test-share 健康探针不对应真实 Share。

HTTP/SSE、WS 每次 response.create、HTTP fallback、Cursor 内部生成和其他实际重试都携带真实执行指纹。WS 保守合并连接内工具集合及非法控制状态，每次生成按当前策略重新检查。previous_response_id 根据 Router URL/installation/Share/用户/App/binding 恢复 24 小时上下文；缺失或跨 scope 拒绝。无法可靠恢复的 Gemini cachedContent 拒绝，不能将未知历史工具当作空集合。

能力关闭后不取消已准入请求，不重发请求；旧视频任务仍按原身份查询和结算，不产生新生成。新的 remix/extend 等未实现入口不会因状态查询而取得生成许可。

## 预览、确认与保存

控制接口 `/_ctl/preview_share_capabilities` 和管理员 Web invoke `preview_share_capabilities` 调用同一个只读域服务。输入是配置 revision、目标 policy、目标用户 policies、是否替换普通策略和 Share 总 Token 上限。预览不发上游请求。

返回逐 App/操作执行支持、可控状态、模型范围、估算原生单位、资源价格、scope/unit 覆盖、blocker、逐项未覆盖确认、15 分钟 HMAC 凭据、冻结计量明细与异常隔离证据。浏览器拒绝旧登录身份、旧 Share/revision 或非法结构的回复。

准入错误在 Claude/OpenAI/Gemini、Web 管理与 Router 预览保留明确机器码；异常隔离不能被自动重试当作临时上游故障。

HMAC 绑定当前 Share/revision、规范化策略、完整目标用户策略、价格修订与确认条件摘要。保存重新计算和校验。Server 写审计 actor/time，用户提供的审计字段不能制造许可。能力、用户额度与回执同 SQLite 事务落盘；操作 ID 同 payload 重放先返回永久回执，异 payload 拒绝；已删除 Share 不复活。

Server Web Bundle 新建 Share 先以能力全关创建，再根据实际绑定预览开启；尚无不落盘的创建前运行时草稿预览。两端显示的旧资源总开关改为只读解释，v1/v2 价卡是内部计量协议，不再作为用户可写的能力总开关。

## 单位与计量

| 当前额度 | 扩展消耗规则 |
| --- | --- |
| Token | 真实模型 Token 照常扣减；图片张数/秒数/工具次数不伪造为 Token。有限 scope 的未覆盖部分需要 owner 逐项确认 |
| USD reject | 内部 v2 资源价卡；实际模型、参数资源价缺失时拒绝，不允许风险确认豁免 |
| USD zero | 显式保留旧策略；缺价维度记未定价，已知文本和其他资源继续精确计费；未覆盖 scope 要确认 |
| 无限额 | 显示没有额度约束，不称为完整额度覆盖 |

新选 USD 能力默认 reject/v2；既有显式 zero 不被后台改写。priced、explicit_free、missing 分开；显式零价合法，不把无价格伪装免费。请求资源数量可能使用版本化估算，预占仅为请求级软上限，不能保证供应商绝不超量。

部分模型/资源缺价不会抹掉整笔已知费用。资源有价但文本 tier 缺失时，资源仍可预占/结算，文本单独标记未定价。响应仅给 ID 而缺数量时继续保留对应资源风险，可信后续证据可修订；不按缺量归零。Token/USD 往返保留历史消费、人工校准和在途风险，能力变更不改变额度起点。

近期展示来自原 attempt 的冻结费率、revision、资源量、估算和金额，避免按当前目录重价。owner 或无 USD 账本的执行显示未记录美元扣减，不制造 $0。美元指 API 等值额度，不是 OAuth 订阅真实花费，也不是钱包余额。

## 持久化、异常与恢复

核心 schema 8 追加四表：`share_capability_admissions`、`share_capability_contexts`、`share_capability_incidents`、`share_settings_receipts`。迁移校验 SHA-256，未来版本拒绝，旧 ledger 不重写。admission 保留 30 天，续接 24 小时；热路径每次最多各清理 100 条过期索引，永久设置回执和异常证据不跟随短期报表清理。

观察到未计划的正资源维度时，incident 与原始用量同事务写入，隔离该用户/App/binding 的后续请求。管理预览展示 scope、资源、原 request 和绑定指纹。不能靠删证据或确认风险解除控制缺陷；先修复 Driver/识别路径，再升级 Driver/目录版本或修正执行绑定，预览并确认新指纹。旧消费和 incident 保留供复核。目前不提供绕过隔离的一键清除入口。

存储写入失败保留原用量发布/恢复门槛；不静默丢弃证据后继续收费。原 USD 对账、restore fence、异步任务和保守结算不变。

## 市场与隐私

新 v13 Share Market 报价冻结 schema 3、空 allowedOperations、measurementPolicy=text-tokens-v1，首版仅出售基础模型交互；理解输入继续可用。managed grant 是明确空集合，不能继承以后开启的媒体权限。旧活动合同没有可证明集合，直接采用新策略被 MARKET_CAPABILITY_CONFLICT 阻止；先走已有合同结束/迁移流程，不追改历史合同和资金。

管理页才返回全部 scope、确认审计与执行绑定。普通用户仅见自己的有效操作集合。Router 私有端点均 private/no-store；Server 管理命令沿用管理员会话。原有账号 ID 可见性规则不因能力展示改变。

## 发布、回退与验收

先升级 Router，再升级 Server；核对 reader13、shareCapabilityPolicyV1、committed schema8、ACK/active 价目。逐 Share 预览采用并检查真实请求入账与耗尽行为，旧兼容态单独登记。安装新版不能当作完成采用。

回退保留能读 v13/schema8 的执行版本，先停止新采用；不删字段降成 v12、不删账本，不通过旧备份抹去升级后消费。真实 Provider/App/模型流量、生产采用和既有灰度对账门槛继续 live_pending。本轮使用本地模拟上游、Rust 持久化/并发回归、两端组件与浏览器测试，最终数字及 review 结果见 Router 能力实施计划的完成记录。
