# Share 额度加固与历史会话续做

> 状态：权威；2026-11-18。本地实现与验证记录，真实供应商与部署仍 `live_pending`。线格式以 Router `PROTOCOL.md` 为准。

## 实现进度核对

历史 `2026-11-17-135315-this-session-is-being-continued-from-a-previous-c.txt` 停在整体 review：两个 reviewer 均因上游限流提前结束。Router 已落入 `1ce2012` 的部分合同与 UI 不能证明 Server 已完成；Server `vq-hardening` 工作树仍保留未提交改动。随后 `91520a2` 的独立能力实施覆盖了部分文本／资源分维度处理，但没有包含历史批量同步、存储维护与第一方错误证据。

本次以该能力版本为基线补齐缺项，保留现有 Codex／OpenCode Zen／价格工作，未覆盖历史工作树。当前选择和取舍如下。

| 原问题 | 当前实现 |
| --- | --- |
| 完整上游错误扣满输出预占 | 实际 HTTPS vendor 域名 + 对应 Profile + 完整标准错误体 + 无 usage／输出才认定；400/403/404/413/422/429/529 免费，其他 5xx 只估输入。中转与无法证明的错误保持保守 |
| 图片／文档理解与资源缺价 | 图片／文档输入走 Token 文本计量；供应商工具及音频等仍独立形成资源计划，v13 能力权限继续强制。zero 只免缺价维度，reject 发网前拒绝；确定策略错误返回非重试 400 |
| 每 attempt 全量别名与无限明细体积 | 保存相关别名族，存量每页 100 行迁移；结算只读一次元数据；旧终态详细元数据无损压缩，保留所有金融与单位转换证据 |
| 准入全局串行／历史查询 | 合并检查点载荷与哈希的范围查询；同步 SQLite 读写移到 blocking 池；status/time 与未整理修订索引支持维护。保留 shares gate、完整行数／哈希校验与事务原子性 |
| 缺价策略与旧 reader | zero/reject 显式配置，保留已有策略和补价追溯规则；不向旧 reader 发布删去 reject 的弱化 descriptor，要求升级支持 `valueQuotaUnpricedPolicyV1` |
| 水位容量 | 签名批量上报：64 项／192 KiB 双上限、逐项唯一 ACK、持久 ACK 后抑制重发，失败项重试；5 秒合并推送、5 秒网络预算、公平游标；旧 peer 16 单项兼容 |
| 窗口边界与并发 | 自然窗口早到请求夹到准入计量起点，账号来源时间不挪动；两端 USD 编辑并发 1–4，保留未编辑历史值与 Token 高并发 |
| 双仓价格漂移 | 三份共享黄金向量固定 SHA-256，两个仓库各自测试；更新向量需同步两边的内容与摘要 |

## 财务证据与恢复边界

schema9 追加维护索引，并从原始归属证据派生虚拟列 `account_started_at_ms`、重建对应账号时间索引；不重写证据 JSON 或其校验摘要，已发布 schema8 的迁移与校验和不修改。终态旧修订可删除冗余版本，永久保留当前金额、最高修订与观察摘要。90 天终态详情采用 `zstd-v1:<sha256>:<base64>`，读取时校验并限制输出／窗口内存为 4 MiB；游标分页也扫描已压缩行，防止每轮维护从头扫描。损坏详情原样保留并报警，不掩盖为合法归档。

attempt 身份、检查点、quota_metering_facts、能力回执和转换审计均不按期限删除。原因是 lifetime 单位往返、幂等重放和迟到观测仍会读取它们；删除会让累计消费失真或失去重放证明。压缩后仍能重新结算并接收更高修订。本次没有 `VACUUM`，不会自动减少 SQLite 文件大小；永久证据仍随请求数增长。

旧快照回收额外保留 attempt 引用、quota_history_preparations、当前／待生效价目、补价重算任务与每个 installation 最新三版。事实的 `migration_revision` 仅在 materialize 成功后写入，同一事务已经生成相应 attempt 引用。

没有引入持久写连接、拆 shares gate、跳过校验或滚动金额缓存：存储恢复可替换数据库文件，策略编辑必须与准入串行，检查点完整性不能因性能优化失效。当前改动减少重复工作而不改变这些约束。历史缓存命中率也不能证明下一次请求会命中，因此不用它降低预占；图片载荷的估算单独处理。UTC 周期和请求级软上限语义不变。

## 整体 review 修正

1. 历史方案只凭状态／Profile 免计量，改成验证完整第一方错误证据；失败标记保存到日志，守卫、日志、Token 影子历史与后续补价保留相同 NoOutput 语义，不能重新加上工具／输出预估。
2. 历史按期删除 attempt 的方案会影响 lifetime 转换、重放和迟到结算，改成无损压缩，金融证据不删。
3. 批次不仅限制数量，还限制实际序列化字节；重复／遗漏／失败 ACK 不确认，ACK 写库失败或同步锁竞争会重新排队。
4. 多个布尔能力元组改为具名结构，避免数组下标碰撞导致批量能力意外打开 Share 权限；Reject 不能投影为 Zero。
5. 归档使用状态与身份游标；旧修订整理使用部分索引，避免已处理历史拖慢每次维护。解压同时限制输出和 zstd 内存窗口。
6. 并发校验覆盖单条、批量、预览与保存；混合批次保留仍在执行的 USD 限制，不暗改未勾选的策略组。
7. 自然周期授权也可能携带账号来源。跨边界准入调整自然窗口金额时间时，原始账号归属时间必须独立保留；账号周期的金额、Token 证明、在途预占、缺证据判断、并发、校准及补价均改用原始时间。后续切换到账号周期不能将上一周期的请求挪到下一周期，也不能因两个时间不同而错误阻止转换。虚拟列与覆盖索引支持这一查询，升级回归验证原始证据与摘要逐字节不变。

## 本地验证

以下为主工作区合并后实际执行的结果，各套件存在重叠，不累加成一个总数：

| 范围 | 实测结果 |
| --- | --- |
| Server `cargo test --locked --all-targets -j 2 -- --test-threads=4` | 单元测试 4,541 通过、3 项忽略；集成测试 139 + 1 + 1 通过，0 失败 |
| Server 严格 Clippy | `cargo clippy --locked --all-targets -j 2 -- -D warnings` 通过 |
| Server 构建与启动 | `cargo build --locked -j 2` 通过；`PORT=18188 scripts/smoke/smoke-local.sh` 通过，覆盖 health、version、内嵌 Web、初始化、密码／API Token 登录及 Provider／Share 创建；临时服务与配置已清理 |
| Rust 格式 | `cargo fmt --check` 与显式列出全部 `src/**/*.rs`（包括 include 文件）的 rustfmt 检查通过 |
| Server Web | 59 个测试文件、416 项测试通过；类型检查和 Vite 生产构建通过，`web-dist` 已同步 |
| Server Node 审计 | `node --test scripts/audit/*.test.mjs` 2,205 项通过；smoke 契约 13 项通过 |
| Server 专项审计 | 官方 Provider writer 生成／检查与 6 项回归，以及覆盖矩阵、USD 计量、SQLite 边界、文档索引、产品／Web 边界、来源、i18n、运行时和产物体积审计通过 |
| Router 前端 | Share 设置 24 项、USD 额度 46 项、Node 审计 12 项通过；类型检查通过；Share 设置真实浏览器脚本 7 个场景通过 |
| Router 构建 | Next `build --webpack` 通过；Google Fonts 使用官方离线测试入口与本地字体夹具，无产品字体源码改动；测试产物不部署 |

本轮 review 发现的问题均已修正并回归，未发现尚未解决的阻断性代码问题。测试使用本地 SQLite、loopback HTTP 与固定夹具；没有真实付费请求、远程变更或部署，也不监控 GitHub Actions。其他会话独立提交的 Codex／Zen／定价工作已保留。上述验证阶段没有创建提交或推送；提交与推送按后续指令单独进行。

离线 `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` 的结果为 `failures=0`、`decision=blocked`、`verificationState=blocked_inputs`。缺少 8 个真实输入且未部署，聚合脚本内部测试明确跳过而产生 `local-contracts-unverified` 标记；上表是独立实际运行的本地检查，不能用它替代真实灰度或宣称发布门禁通过。
