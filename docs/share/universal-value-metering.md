# 全供应商美元等值额度

> 状态：权威实现规范。本地仅静态验证，产品回归由 GitHub CI 执行；生产灰度独立待完成，全部真实 Provider/App 验收保持 `live_pending`。未部署。

美元表示平台执行价目中的 API 等值用量。OAuth 订阅、积分、百分比和实际成本不参与美元换算；钱包、租金和 Provider 收益沿用原合同。已有 Token 和 v1 策略不会因升级而改变。

## 支持与就绪

Provider Registry 是运行时权威：39 families、91 Profiles、28 Drivers，Custom HTTP 展开 99 个 Profile/Driver 组合。所有支持转发的组合均提供美元额度选项，API Key、OAuth、Coding Plan、Web Session、Legacy 不作为身份白名单。

`value_metering_strategy` 声明 Driver 生命周期责任。`scripts/audit/audit-value-metering.mjs` 从 Registry 生成 `assets/contract/value-metering-coverage.json`，约束策略和关键钩子。它是静态覆盖清单，不能证明 99 对已在真实上游执行通过；表中的 transport 是现有入口分类，legacy 以最终解析的 Driver 为准。

单用户、批量、市场席位和美元试用共用规则。`$` 可选择并保留草稿，保存与准入检查存储、Router 协商、已激活价目、模型映射及操作资源价格。已支持、配置是否就绪、本次用量精度分别诊断。模型、资源或档位缺价只拒绝相关操作，不将整类 Provider 标为不支持。

## 生成与结算

每次实际生成先确定最终执行模型、档位和资源计划，再在 committed SQLite IMMEDIATE 事务中持久准入。HTTP/SSE、原生与跨协议入口、各专用 Driver、WebSocket 的 response.create、HTTP fallback、内部摘要及 Cursor 内部生成都使用同一金额账本。免费模型目录、countTokens 等辅助操作不产生金额 attempt，继续执行身份与授权检查。

每次新生成具有独立 attempt；认证刷新和连接恢复分别处理。重附着已有响应不新计生成，HTTP 降级或重放产生新生成时重新准入。重试前同步结算上一 attempt，已有 usage 先写回，不能清空后再结算。内部摘要有独立 request ID、父请求关系与市场 receipt，不重复增加用户请求计数。

DeepSeek 失效缓存会话重建后的再次生成也独立准入；创建会话、POW 和凭据准备不产生金额 attempt。Web Session、Qoder、Trae 的已发送标记在实际生成发送前设置，准备失败按未发送结算。

响应生命周期守卫覆盖正常终结、错误返回、取消、超时、提前关闭及从未读取 body。只有未发送或明确认证拒绝且无任何用量证据可记零。错误 HTTP 状态、EOF、未看到输出本身都不能证明免费。可信 usage 优先，完整、估算、中断与未知分开；未知按 `max(已确认金额, 原有金额, 有界风险)` 保守结算，迟到可信证据允许修订。

继续复用请求级软上限、并发 `min(用户限制,4)`、预占、窗口聚合、修订和水位。跨策略 epoch 不清零；scope 包含 Router/installation/Share/用户/市场 entitlement。手工校准、恢复 fence、检查点、试用消费下界不因资源扩展失效。

## 资源与精度

v2 为模型增加 `resources` 映射，值为每个原生单位的整数 picoUSD 字符串。模型可只有资源价而没有文本价；文本操作仍必须有该档位的文本价格。exact alias 由管理员显式配置，不用订阅积分猜测价格。

| 资源事实 | 当前规则 |
| --- | --- |
| 图片生成/编辑 | 请求 n、size、quality 形成计划；结果数量或稳定生成项 ID 提供证据 |
| 视频异步任务 | duration/resolution 形成毫秒资源计划；任务持久关联原 request/attempt/scope，轮询没有新金额 attempt |
| 音频内容/输出 | 读取 OpenAI audio token details、Gemini modality token details；缺缓存细分标记 estimated |
| 内置付费工具 | 声明 web search/fetch、file search、code execution、computer/MCP 等维度；读取上游累计计数或稳定调用 ID |
| 普通 function/custom 工具 | 客户端工具定义及返回不视为新付费工具，文本仍按 Token 计量 |

计划中的图片最多 100、视频默认 15 秒且最高 3600 秒、每个工具维度保守界限 8 次、输入/输出音频各 128000 Token；这些是带版本的预占/缺证据估算，不是上游执行硬上限，也不宣称精确计量。已观察数量不被这些界限截小。输入图片在普通文本请求中的 aggregate Token 继续按模型价，不能凭图片个数再次加收未声明单位。

Observer 在协议转换前采集证据；累计计数取最大值，稳定调用 ID 去重，最多保留 1024 项。无法证明精确的 Gemini grounding query、url、代码及图片事件计数均为 estimated。精确 audio 组成从 aggregate fresh input/output 中扣除一次，长上下文档位按原始完整 context 判定；估算界限不能从已观察文本中抵扣。

估算或部分资源计数不能降低请求的保守界限，采用观察量与该维度界限的较大值；只有可信证据可以下调。发送后发现正数量但缺价的资源时保留原预占和原始用量，隔离该用户窗口等待对账，不影响其他用户。

价目 domain 可表达图片、视频/音频毫秒、图片/音频 Token、缓存和内置工具等维度，但只有现有 adapter 真正输出的事实才是精确证据。支持某个价目 key 不等于所有 Driver 都提供该 key 的精确 usage；其余使用明确标记的保守界限。没有新增独立 audio API 入口。

资源量与价格缺失分别处理：缺价在发送前拒绝，显式零价合法；计划有价但结果缺证据时保留版本化 bound。未知、矛盾、解析失败不能作为 observed。新发现的正资源维度不得按零忽略；定价不成立时保留风险并要求相关配置/对账。

## 异步与恢复

Server schema 5 的 `value_async_tasks` 保存 task 到 scope、原 request/attempt 的关联。有效 pending task 不被普通超时清扫结为零；过期转为未知结果，之后可信完成仍可修订。轮询必须同时匹配任务、父请求、Share 和用户，终态内容按摘要幂等。

完成后迟到失败不能回退；失败本身可能已消耗资源，因此保守保留风险，之后完成可更正。原生成资源计划不会被一般历史压缩清掉。轮询结算和 usage 在一个事务完成，故障隔离回到原生成窗口，市场试用镜像也同步原请求。修订下调周期金额不会重新开放已经消费的一次性试用额度。

## 协议与价目管理

文本扩面沿用 v10 / `api-equivalent-v1`。资源策略使用 v11 / `api-equivalent-v2`、价目 schema 2、Router heartbeat `resourceValueQuotaV2`；signed fetch 发送 `supportedTariffPolicies` 后才下发 v2。旧 Server 继续获得 v1，不能静默降级现有 v2 授权。签名时间较旧的能力声明不覆盖较新的记录，v2 ACK/active 要求存储 schema 5。

Router 迁移 111 并行保存两类价目及独立资源价格编辑历史。管理页以整数换算每张/次、每秒、每百万 Token，支持模型资源价和 exact alias，CAS 防止覆盖并发修改，冲突保留草稿。两个策略的价格与 alias 从同一当前目录派生并在事务中发布；不可变 hash、生效延迟、ACK、active receipt 和历史冻结保持原语义。

Router 永久保留每个资源修订的类型标记，历史快照正文归档不撤销 schema 5 的 ACK/active 要求。新建或 Token 草稿默认保留文本策略，资源计量仅在 owner 显式勾选时开启；既有 v2 草稿保持原选择。

市场席位及 subscription 保存 `value_tariff_policy_id`，报价、租用、续约、试用与转换使用冻结合同策略。修改席位不会改已有租约的计价策略。普通/批量/市场编辑器均保留原 v1 选择，资源策略须显式选择，不能由升级后台自动打开。

## 验证与发布

CI 应执行新增的资源计划/证据、audio 去重及长上下文、持久账本修订/取消/异步重启、双价目下发/旧签名/alias CAS、市场冻结和精确 UI 金额回归，连同现有 Driver/跨协议/Token/窗口/试用套件。本地仅编译测试目标和执行类型、格式、静态审计，不执行产品测试、浏览器自动化、截图或真实模型请求。

先发布 Router，再发布 Server；等待协商、资源价格和 active receipt 后显式启用新策略。`woegum--f11pf11rf7` 的 Claude/Codex OpenAI OAuth 两绑定须在生产独立验证：预览、保存、实际请求入账及耗尽拦截。每个开放面继续满足既有 7 个自然日、至少 100 条完成请求的灰度对账门槛，缺输入不得标为 live_verified。

回滚先停止新策略启用、保留全部账本及快照，v2 合同须继续由支持 v11/schema 5 的版本执行；不可直接降到无法读取现有合同/数据库的旧二进制。没有追溯猜算历史美元、在线重建账本或自动切回 Token。

## 整体 review 记录

本轮 review 修正内部重试漏 attempt、准备阶段过早标记已发送、用户间资源 readiness 干扰、定时策略缺价激活、异步失败与估算资源计数低估、发送后新资源缺价以及历史 v2 ACK 版本依据丢失。新增回归进入 CI，包含 schema 5 回执、资源修订和窗口隔离；本地只编译目标。

接续 CI 修正迁移表数和未来版本断言、SQLite authority 激活顺序及 DeepSeek 完整执行模型价目夹具。Web Session 缺价按执行模型诊断，WorkBuddy 不支持的图片由模型能力校验拒绝；非认证限流且无用量证据仍按未知保守结算，相应回归按这些规则断言。

复核状态写入仍经 ServerStateInner 域方法封装，未新增存储锁逆序、Provider 凭据白名单或钱包写入。严格 Clippy（全部目标）、两端类型检查、include! 源格式、Provider writer 证据、Registry/USD 覆盖、SQLite authority、Web 边界和文档索引静态检查通过。发布门槛仍包括 CI 产品回归与上述真实灰度，不能以静态覆盖代替。
