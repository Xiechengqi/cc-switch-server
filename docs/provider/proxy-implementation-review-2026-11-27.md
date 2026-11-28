# R6 / R6F 实施与整体 review

> 状态：本轮 21 项离线实施、条件研究与整体 review 完成。以下验证绑定明确的完整源快照；真实厂商接受性保持 `live_pending`。实施入口与逐类记录见 [增强总账 §24](../../cc-switch-server-enhance.md#enhance-r6-implementation-20261127)。

范围是 [R6 十项](proxy-comparison-plan-2026-11-27.md) 与 [R6F 十一项](proxy-provider-focus-plan-2026-11-27.md)。保持原计划允许的条件研究退出，不把缺少独立协议依据的能力标为已开放。既有 R5 不重做；并行主仓改动保留，本回执验证的是下述隔离快照，不能扩大到其后新增的 Client 健康同步修改。

| 条目 | 当前实施或研究结论 | 权威证据 / 待保留边界 |
| --- | --- | --- |
| R6-ZEN-01 | 统一三个既有推理文本别名、逐帧去重、空白/累计、首语义与续写选择 | [Zen 资产](../../assets/contract/r6-opencode-zen.json)；不遍历工具 JSON 或密文 |
| R6-TR-01 | 原生显式关闭不再当作省略；无已审关闭能力时发送前拒绝 | [Trae 资产](../../assets/contract/r6-trae.json)；省略保持未知上游默认 |
| R6-TR-02 | 条件研究退出：Hub 行式 mock 不能替代同端点/function/client 的独立厂商样本 | 正式入口重放明确失败、单次生成；保留既有具名方言 |
| R6-TR-03 | 条件研究退出：模型→function→绑定目录的权限关系未获独立证明 | 固定 work function，未知组零生成，不试探换组 |
| R6-TR-04 | 条件研究退出：依赖前两项及图片理解证据 | SOLO 图片继续拒绝；raw 图片保留，不扩 Registry/Share 权限 |
| R6-QD-01 | 新嵌套目录条件研究退出；另修复已证实的本地原生关闭/预算解释 | [Qoder 资产](../../assets/contract/r6-qoder.json)；旧 CLI oracle、high→max、context 位置及 chat/deny 保留 |
| R6-OBS-01 | Trae/Qoder 均从规范控制与最终 prepared payload 记录 requested/effective | 原有日志/Router 字段；准备不等于已发包/执行，无档位开关与未知值不造具体 effort |
| R6-EVID-01 | 原 conformance 增加可选只读操作视图与精确 scope 查询 | [数据](../../assets/contract/provider-conformance-evidence.json)；9历史/5fixture/live0，粗门禁不变 |
| R6-CORE-01 | Zen 缓冲解码生命周期迁为真实 Rust 模块，初次/续写均调用 | [模块](../../src/proxy/forwarder/opencode_zen_document.rs)；单职责迁移，wire/owner/结算不变 |
| R6-ZEN-02 | 已执行正常与私有 cgroup 压力实验，保留负向容量结果 | [资源资产](../../assets/contract/r6-zen-resources.json)；128MiB/384MiB-high 为 typed504，未发生 OOM；不声称性能收益 |
| R6F-GR-01 | 清洗限制到已识别协议层；schema/业务值及工具正文不被全树删除 | [Grok 资产](../../assets/contract/r6-grok.json)；HTTP/WS 正式反例及工具别名控制 |
| R6F-OA-01 | Codex 上游 WS 缓存 key 在最终握手准备后形成，协商与租户隔离 | [Codex 资产](../../assets/contract/r6-codex-oauth.json)；仅专用闲置 socket 缓存，关闭/未命中无旧握手复用 |
| R6F-BD-01 | prepared catalog 带世代 lease，替换/撤权在正式发送前复核 | [Buddy 资产](../../assets/contract/r6-buddy.json)；缓存压力/淘汰不丢撤权通道，不把 TTL 当在途撤权 |
| R6F-AG-01 | 每响应稳定唯一的缺失 ID 回退；保留合法原 ID、grounding use/result 配对 | [Antigravity 资产](../../assets/contract/r6-antigravity.json)；不改变上游函数 ID 或签名权限 |
| R6F-CL-01 | 条件研究退出：没有独立 Advisor 密文 owner 规则依据，继续 opaque 透传 | [Claude 资产](../../assets/contract/r6-claude.json)；不建猜测 owner cache、不静默删历史 |
| R6F-AG-02 | 无名未知调用失败；已知显式 ID 可继承确定名称，冲突/迟到参数锁存错误 | 保留合法名称 tool、空参数和签名；不猜最近工具或合成可执行调用 |
| R6F-BD-02 | 条件研究退出：参考身份版本/请求头关系相互冲突，保留受审 wire | 产品/站点/login 与 CLI 数据面继续分开，不借版本号替换完整身份 |
| R6F-GR-02 | 条件研究退出：新版 Build 身份与现有 direct-XAI rail 不同 | 不只更新 UA/version 字符串或混入另一端点 |
| R6F-GR-03 | 条件研究退出：缺同 rail 的明确模型级限流证据 | 保留通用账号限流；未知 429 不推断模型权限或触发新生成 |
| R6F-OA-02 | managed Responses 接入受限 tools/usage.attribution 裁剪协商 | 保留核心用量/工具生成，拒绝宽 usage 与未知值；不推广到模型/search/普通兼容 Provider |
| R6F-OA-03 | HTTP/SSE/WS/fallback 组合坏图错误补证，保留已有不重放行为 | 不声称新增坏图分类器；成功发送后继续禁止透明重放 |

## 横向 review 边界

- Provider/Account/Share 单绑定、代次与 runtime fencing 不被参考账号池替代；所有恢复仍在既有提交边界内。
- 状态写入继续由 state 域方法封装，保持锁序和依赖方向；新测试也使用现有域方法配置绑定。
- 请求语义只在精确协议位置解释；普通工具 JSON、schema、opaque 历史不进入通用递归清洗。
- 诊断只用有限枚举与最终准备值；操作级视图不验证私有 receipt 内容、不运行 harness、不授予能力。
- 各类 fixture 数量存在过滤器重叠，不相加成独立用例数；历史厂商成功不升级当前所有模型/操作。
- 未做 UI 自动化、前端测试、部署测试或真实账号操作。离线 readiness 的输入阻断与本轮本地验证分开报告。

已逐段复核本轮 Grok typed 清洗、Codex 最终握手准备/缓存身份/受限裁剪、Buddy epoch/TTL/末次发送围栏、Gemini ID/工具状态、Zen 别名/解码 owner、Trae/Qoder 原生控制与诊断以及操作证据模块。未发现尚未处理的本轮本地缺陷。Buddy 可选缓存拒绝不保留第二份正文，活动 runtime 仍进入请求内存预约；WS completion guard 只保留固定 32 项摘要，且绑定已有 socket owner。Zen 解码模块的依赖通过具名参数/导入表达，重试与结算仍在原 owner。

复验期间同仓另有 Client 身份恢复的并行改动，涉及 `state.rs`、`state/background.rs`、身份恢复合同及生成的 writer inventory，均保留。完整复制并逐文件校验后，在 `/tmp/cc-r6-final-integration-20261127` 固定验证输入。最终与主工作区对照，只有 `state.rs` 的四个 Client 健康/Share 同步方法及对应生成清单不同；R6 Provider 源码、三处测试维护和共享文件内的 Buddy 围栏一致。后续方法差异为 `flush_one_router_model_health_refresh`、`reconcile_router_share_after_failed_claim`、`sync_one_share_to_router_locked`、`sync_one_share_to_router_locked_with_policy`，只审查了与本轮的交界，没有把其后的修改计作本回执已验证。

总静态脚本原含前端 typecheck/test 段；本轮按后端范围使用仓外副本移除该段。两组 Node 测试在同一快照单独执行，副本中的对应两行改为已单独执行的提示，其他静态门禁保留。仓内脚本不变；最终入口为 `/tmp/cc-r6-final-static-components-v9.sh`，日志为 `/tmp/cc-r6-final-private-static-v9.log`。本报告不声称原始总脚本已整段运行。TypeScript 5.9.3 只用于原有静态 AST 审计，依赖来自本项目现有安装目录。

## 最终验证

源快照基于 `73cf158f147d402abed66150dbd5ad77c49e033d`，包含 1225 个输入文件，摘要为 `c849d724464d44af770f40154bcf41eab4e526137555183b99e82069af034083`。算法为排序后的仓内路径/SHA-256 列表的规范 JSON 摘要，覆盖 tracked/untracked 非忽略文件及 `web-dist`，仅排除本报告、总账、协议证据和文档索引四个最终报告文件。验证后再次逐文件核对不变；主工作区并发前进到 `848be22958ee1499d02ce374f571b0241b558dc3`，不能用该 HEAD 替换本回执的构建基线。

| 检查目标 | 通过 | 失败 | 忽略 |
| --- | ---: | ---: | ---: |
| Rust library 全量 | 4727 | 0 | 6 |
| `api_contract` | 140 | 0 | 0 |
| `cursor_fixtures` | 1 | 0 | 0 |
| `us04_lease_payload_contract` | 1 | 0 | 0 |
| 主程序测试目标 | 1 | 0 | 0 |
| Rust doc-tests | 0 | 0 | 0 |
| 后端 Node 审计 | 2216 | 0 | 0 |
| smoke 脚本单测 | 13 | 0 | 0 |

Rust 按不同 test target 合计 4870 项通过、6 项原有 opt-in 忽略；没有叠加类内过滤器的重叠数量。Rust 1.94.1 / Cargo 1.94.1、Node 26.13.1；Rust 测试使用 `RUST_MIN_STACK=67108864` 和单测试线程。`cargo fmt --check`、显式全部 src/include Rustfmt、`cargo check/build --offline`、严格 `cargo clippy --all-targets -- -D warnings` 以及其余后端/静态门禁通过。21 个唯一计划编号均有实施或允许的条件退出记录；OBS 同时覆盖 Trae 和 Qoder。

完整 library 的实际执行二进制 SHA-256 为 `04f66b5ae885c515ab09397fecf7b42d8709e20e21025f4ac768988d0db06788`，运行时从 `/proc/PID/exe` 保存，其 DWARF 工作目录确认来自该隔离源。预编译候选 `35f7f148f69dbaeca957df7b2eab000f08de9523adeda2ddaf21320ceceabcb5` 只用于先行两项回归，不能冒充完整执行二进制。共享 `target` 在完整 library 结束前被外部删除，原 Cargo 命令随后因缺失主程序测试文件退出 101；library 结果仍为成功，未把原整段命令记作通过。其余所有测试目标、构建和静态检查在独立 `target` 中补齐，源码保持同一摘要。library 为 debug-info 2，其余目标使用未优化、debug-info 0 的 dev/test 配置控制资源占用。

整体运行发现并维护了三处已有测试：并发 setup 保留单一密码/hint 的断言，改为显式执行后台通知；连续心跳 401 保留失败阈值与注册次数断言，并验证 Client 未就绪时 Share 同步仍待处理；API setup 验证通知异步返回、尚无投递、初始化和密码/hint 保留。后续 503 投递失败继续由既有 state 回归覆盖。修改前两项库测试和一项 API 测试确实失败，修正后的单项及完整目标均通过。`setup.rs/state.rs` 测试模块之前的生产源码经字节比较不变；writer inventory 按两份实际源码分别通过生成脚本更新并验证。

本地 HTTP smoke 通过 health/version、静态页 HTTP fallback、初始化、密码登录、API Token 轮换/登录、Provider 和 Share 创建。使用动态 `127.0.0.1` 端口及独立临时配置，结束后配置删除、匹配进程为零。隔离 worktree 的 `.git` 是文件，而既有 `build.rs` 监听 `.git/HEAD/index` 并生成动态构建时间；日志显示 `cargo run` 再编译约 30 秒，与原 smoke 的 30 秒启动等待冲突。最终只在仓外副本把启动命令替换为预构建二进制，逐字核对其余 HTTP 步骤不变，仓内 smoke/build 脚本未改。实际启动二进制 SHA-256 为 `77f424259e7625da313c2c14dd4aac74ceee7d32ee5d405e68f8532f99d23086`；仓内脚本摘要为 `1eb78148499079cfc5ffa54200d818e92d50621056b5ce9b12ab9a85be025f3e`。这证明本地 API 流程，不是原 `cargo run` 启动方式或部署验收通过。

离线 readiness 按 `RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 实际返回 `blocked_inputs`（退出 1）：本地检查失败 0、警告 2，8 项真实输入与未执行部署合计 9 项外部阻断，另有内嵌测试未运行的 1 项内部阻断。另行执行的上述测试结果独立报告，不改写 readiness 的判断，也不宣称 release-ready。

早期工作区漂移、宿主 OOM、链接器 SIGBUS、共享缓存删除，以及未成功启动的 smoke 尝试均未计为通过。最终日志与汇总在 `/tmp/cc-r6-final-validation.json`、`/tmp/cc-r6-final-requirement-audit-v9.json`；Rust/Node/静态与 smoke 的具体日志路径一并记录。Zen 私有 cgroup 压力实验的 typed 504 / probe exit 101 保留为容量限制；未承诺吞吐、延迟或 RSS 改善。

## 总体结论

21 项离线实施/条件研究及整体 review 已收口，未发现尚未处理的本轮 Provider 缺陷。八项证据不足的研究保留原边界，不能理解为新能力已启用；细粒度视图仍为 9 历史 / 5 fixture / 0 live，所有相应真实 operation 继续 `live_pending`。未运行 UI 自动化、前端测试或部署测试，本轮执行未发起提交或推送。
