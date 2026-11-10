# GitHub Copilot 单账号反代合同

状态：本仓实现合同。真实 github.com / GHES 账号的 device flow、模型权限、配额与推理仍为 `live_pending`；本地 fixture 不代表真实订阅或收费接受性。

## 身份与端点

Provider 固定绑定一个 `GitHubCopilot` ManagedAccount。GitHub OAuth token 留在控制面，只把 exchange 得到的短期 Copilot token 发给数据面。生产 origin 必须经过既有 HTTPS、域族、无 userinfo/query/fragment、根路径校验，优先使用该账号 `endpoints.api`。不能从模型描述、客户端正文或 Header 指定任意 origin，不能跨账号、Provider、GitHub domain 或产品 fallback。

模型发现按 account id、authIdentityGeneration、账号 token generation、内部 token generation、GitHub domain 和 validated API origin 缓存。发现 API 可在允许的临时失败下返回同 scope stale 观察结果，但**推理权限只接受 fresh 目录**。成功的空目录表示无可调用模型；policy/picker 禁用的模型不能借静态表或缓存恢复权限。重复 ID 不合并授权：明确 deny 或矛盾 capability 拒绝该模型，无元数据 bare ID 只作重复占位而不覆盖完整描述。

## 模型能力与协议选择

运行时先按 Provider 权威单模型/显式模型映射及已审查拼写 alias 取得实际模型，再读取绑定账号的目录。精确模型必须存在；模型名字包含 Claude/Codex 不决定端点。只识别下面三条固定路径：

| 上游协议 | 固定路径 | 原生优先的下游 |
| --- | --- | --- |
| Anthropic Messages | `/v1/messages` | Claude Messages |
| OpenAI Responses | `/responses` | Codex Responses |
| OpenAI Chat | `/chat/completions` | Chat、Gemini bridge |

多协议模型优先原生；若不支持下游原生协议，使用已审查转换。旧目录模型没有 supported_endpoints 时保留既有 Chat 合同，但前提仍是模型存在于 fresh authenticated catalog。显式空数组、null 或错误类型不属于字段缺失，不能开放 legacy Chat。未知路径、绝对 URL、query、空/被禁模型或只有 stale 缓存，均不触发协议猜测或网络试错。

Request plan 保留模型描述、目录有效期以及完整 auth scope，发包前再次检查。token/origin/generation 改变、模型撤权或能力变化会拒绝旧 plan；不把旧 endpoint plan 与新 token 拼在一起发送。唯一 eligible 推理 401 仍可在同账号同 identity 下恢复一次，并重新发现/验证该 token 的模型权限；目录发现的 401 恢复也占用同一请求的 Auth 预算，不能与推理 401 叠加；不增加端点轮询或输出后恢复。

Native Messages 加固定 `anthropic-version: 2023-06-01`，不经 Chat 透传签名 thinking、工具历史和 cache 正文。Native Responses 不经 Chat 透传 encrypted reasoning、previous_response_id 与原生 input。终态检测、错误和流/非流聚合按实际选中的协议运行；不以 EOF 伪造完成。

## 默认请求语义

Copilot optimizer 默认保留身份、分类与 request-id Header。以下正文改变默认关闭，需要各自显式 opt-in：

- `copilotOptimizer.toolResultMerging`
- `copilotOptimizer.stripThinking`
- `copilotOptimizer.warmupDowngrade`（目标由 warmupModel 配置）

只设置 `copilotOptimizer: true` 或 `enabled: true` 不开启这些正文选项。Header 中有 anthropic-beta 且没有 tools，不会在默认配置下将普通请求换成 gpt-5-mini。显式正文选项仍不能覆盖 Provider 权威 single-model 策略。

Anthropic 孤立、重复或空 tool_use_id 的 tool_result 在 preflight 返回明确 400，不静默删掉或改为普通文本。并行结果按原 ID/顺序保留；合法拆到相邻 user 消息的 pending 结果可验证。Chat/Responses 的工具验证归其协议转换层，不猜测文本调用。

已审查的 Claude 拼写 alias 可规范化，例如 dash 版本与 `[1m]`；存在配置目录时目标需精确匹配。未知版本、模型族、窗口不自动换为“最接近”型号；不能把 1m 降级为普通窗口。目录不是授权以外的 fallback 池。

## 代码与验证入口

- `src/domain/accounts/copilot.rs`：固定端点选择与路径负例。
- `src/state.rs` / `src/state/copilot_inference_tests.rs`：控制面 plan 与发送前 scope、撤权和有效期检查。
- `src/proxy/adapters.rs`：一次 preflight、保留权威模型选择、按请求协议转换。
- `src/proxy/forwarder/copilot_native_tests.rs`：opaque 模型的 native 流/非流与刷新撤权 loopback fixture。
- `src/proxy/copilot_optimizer.rs` / `copilot_model_map.rs`：默认语义与 alias 负例。

推荐专项命令（不是自动真实验收）：`RUST_MIN_STACK=16777216 cargo test --lib copilot_`。测试栈用于本仓大型 async 测试，不改变生产 Tokio 栈或请求 policy。新增实现的实际测试成绩记录于根增强文档第 22 节；外部参考只作为冻结源码证据，不参与本仓测试或运行。
