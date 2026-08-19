# cc-switch-server 审查报告（修复前）

> **历史快照 / 已废止**：本文记录 2026-08-06 修复前状态，其中关于 Router Token Market、按 app 售价和旧 Share sale contract 的判断已经失效。当前产品边界以 [`docs/token-market-decoupling-plan.md`](docs/token-market-decoupling-plan.md) 和根目录 `AGENTS.md` 为准；不得按本文恢复旧 Token Market 代码或协议。

- **仓库**：`/data/projects/cc-switch-server`
- **分支**：`main`，HEAD `bb004b0 refactor(server): consolidate runtime files under config directory`
- **审查日期**：2026-08-06
- **审查范围**：① 技术债盘点 ② 多 app(claude/codex/gemini) provider bundle 合并实现正确性 ③ 前端 UI 样式与 UX 交互
- **代码规模**：Rust 211,980 行 / 187 文件（生产 140,961 + 测试 71,202，测试占比 33.6%）；前端 `web-src/src` 204 `.tsx` + 222 `.ts`，约 34k 行（不含 locale）

---

## 目录

- [0. 工作区状态异常（需先确认）](#0-工作区状态异常需先确认)
- [1. 多 app Bundle 实现 Review](#1-多-app-bundle-实现-review)
- [2. 技术债](#2-技术债)
- [3. UI / UX](#3-ui--ux)
- [4. 建议的动手顺序](#4-建议的动手顺序)

---

## 0. 工作区状态异常（需先确认）

会话开始时 `git status` 为 clean，审查过程中出现两个文件被修改，文件时间戳 **2026-08-06 07:22**：

```
 M src/clients/router/tunnel.rs  (+133)
 M src/state.rs                  (+40)
```

改动内容是一个**完整、连贯的功能实现**：

- `tunnel.rs:102` 新增 `pub enum TunnelReplacementMode { Graceful, NamespaceRebind }` 及其 `impl`
- `state.rs` 将 `force_reconnect_client_tunnel` / `force_reconnect_share_tunnel` 重构为
  `replace_client_tunnel` / `replace_share_tunnel`，并透传 `mode` 参数
- `reconnect_after_client_subdomain_adoption`（`state.rs:10034`）改为使用 `NamespaceRebind`，
  原先走 `Graceful`

**这不是本次审查产生的副作用**：三个审查代理均为只读调研，其中做 bundle 复现的代理写过临时测试并已还原，且明确声明未触碰这两个文件。最可能是**另一个并发会话 / 进程**在同一仓库工作。

**状态**：未回滚、未提交、未改动。下述所有结论均基于 HEAD 提交状态，未计入这两个文件的未提交改动。

> **行动项**：确认这两个文件的来源，再开始修复，否则可能与另一路工作冲突。

---

## 1. 多 app Bundle 实现 Review

### 1.1 架构澄清

"bundle" 在存储层**不是一条记录**，而是 N 条 `StoredProvider`——每个 app 一条，共用同一个 `provider.id`，
通过 `provider.extra` 中的 `bundleId` / `familyId` / `surfaceEnabled` 三个字段关联
（`src/domain/providers/bundle.rs:17-19`、`209-217`）。记录键仍是 `(AppKind, provider_id)`
（`registry.rs:56-71`）。家族在 `assets/contract/provider-registry.json` 中声明自己的 surface 集合（1/2/3 个）。

**代理路径完全不感知 bundle**——`src/proxy/forwarder.rs` 与 `src/domain/sharing/shares.rs` 中
`bundle_id` 出现次数为 **0**。Bundle 纯粹是管理 / 编排层的分组概念。这个分层本身是干净的。

### 1.2 已验证正确的部分（覆盖面声明）

| 关注点 | 结论 | 证据 |
|---|---|---|
| **App 解析歧义** | 无歧义。纯 URL 路径驱动，无 header / 模型名推断，无默认回退，未匹配即 404 | axum 路由 `api/mod.rs:548-658` → `ProxyRoute::app()` `proxy/router.rs:26-34` → `forwarder.rs:1148` 读一次 |
| **Header 信任边界** | 正确。无条件剥离客户端传入的 `x-cc-switch-share-id` / `x-cc-switch-user-email` / `user-country` / `request-id`，仅从 HMAC 校验通过的 ingress context 重新注入。**无法伪造 share-id 或用户身份** | `verify_router_ingress` `api/mod.rs:676-709`；注入 `api/mod.rs:711-765`；`require_router_share_ingress` `api/mod.rs:659` |
| **凭证刷新共享** | 正确。刷新锁键为 `(provider_type, account_id)` 而非按 app；`try_lock_owned()` → `lock_owned().await` 单飞。同 bundle 各 surface 携带相同 `auth_binding.account_id`，天然竞争同一把锁。**无双刷新** | `accounts/managers.rs:1066-1068`、`293-315`；`bundle.rs:272-281` |
| **认证失败冷却** | 60s 冷却跨 app 共享——**这是对的**，失败是认证源的属性而非 app 的属性 | `managers.rs:216-229`、`372-378` |
| **用量计费** | 按 `(app, provider_id)` 记账，一次请求写且仅写一个桶。**无重复计费、无漏记** | — |
| **并发限额** | `AccountInFlightTracker` 键为 `format!("{provider_type}:{account_id}")`，即按认证源。**3 个 app 的 bundle 不会拿到 3 倍账号额度**。share/user 限额按 `share_id` + user 计，作用域正确 | `state.rs:1689`、`1750`、`1650-1662` |
| **模型策略（8439d3f）** | `model_policy` / `upstream_model` 只存在于 bundle 作用域，`ProviderBundleSurfaceWriteDraft` 无此字段，per-surface 分叉**结构上不可能** | `bundle.rs:66-68`；`validate_shared_configuration` `bundle.rs:294-316` |
| **单 surface 降级隔离** | 良好。禁用 / 配置错误的 surface 只让该 app 404，不影响同 bundle 其他 app；缺凭证产生 warning 而非 error | `runtime.rs:147`、`proxy/router.rs:76`、`forwarder.rs:14700`、`api/mod.rs:936-947`、`runtime.rs:487` |
| **providers.json 迁移** | S1→S2 有快照 + SHA-256 manifest + 目录锁 + 原子 temp-rename，幂等，带中断恢复；S2 携带 `schemaVersion: 4` 与 `LegacyDecoderRejectGuard`（旧解码器响亮失败而非静默截断）。**刻意没有** legacy→bundle 自动迁移，老单 app provider 保持普通 provider，仅可显式 `adopt-profile` 转换——自洽的选择 | `storage_migration.rs:231-312`；`store_v2.rs:83-103`、`347` |

**观察项（非缺陷）**：
- 无 bundle 级用量汇总，一个 bundle 的用量分散在最多 3 个桶中，需在查询侧聚合。
- `capacity_pool_id`（按认证源派生的身份）被计算并上报，但**未用于任何本地强制**，因此两个走同一 API key 的不同 share 在本地不会合并计额。

---

### 1.3 🔴 CRITICAL — C1：删除 bundle 会连带删除同名的无关 provider（已实证复现）

**根因**：

```rust
// src/domain/providers/bundle.rs:675-677
pub fn bundle_id(provider: &Provider) -> &str {
    extra_string_ref(provider, BUNDLE_ID_FIELD).unwrap_or(&provider.id)   // ← 兜底到 provider.id
}
```

这个兜底使得**每个非 bundle 的普通 provider 都成为"以自己 id 为名的 bundle"**。

`store.rs` 一直很小心，四处都预过滤：

```rust
.filter(|stored| super::bundle::is_explicit_bundle_surface(&stored.provider))
// store.rs:376, 456, 545, 594
```

但 **`state.rs` 的四处没有过滤**：

```rust
// src/state.rs:4713-4738  delete_provider_bundle_command
let surfaces = providers.providers.iter()
    .filter(|stored| provider_bundle_id(&stored.provider) == bundle_id)   // ← 缺 is_explicit_bundle_surface
    .map(|stored| (stored.app, stored.provider.id.clone()))
    .collect::<Vec<_>>();
...
for (app, provider_id) in surfaces {
    providers.remove(app, &provider_id);
}
```

同样缺失于：`state.rs:4662`（delete preview）、`4717`、`4726`、`5034`（upsert）。

**实证复现**（`family.openai_oauth` 仅含 claude + codex 两个 surface，gemini 空出）：

```
bundle "collide" 创建 (claude+codex)          → 200 OK, rev=1
普通 gemini provider id="collide" 创建         → ok=true
gemini 查询（删除前）                          → {"id":"collide", "credentialConfigured":true}
delete-preview                                → {"bundleId":"collide","revision":1,
                                                 "shareIds":[],"blocked":false}   ← 零警告
DELETE bundle "collide"                       → {"ok":true,"deleted":true}
gemini 查询（删除后）                          → {"ok":true,"providers":[]}   ← 连同 API Key 一起销毁
```

**反向顺序同样中招**：先建普通 provider 再建 bundle：

```
bundle create → 409 Conflict
{"error":"expectedRevision is required when updating a Provider Bundle"}
```

因为 `state.rs:5031-5036` 把那个无关的普通 provider 归类为"已存在的 bundle surface"。一旦进入该状态，
`state.rs:5072-5084` 的 familyId 不可变检查（`cc_switch_provider_bundle_family_conflict`，
"Provider Bundle family is immutable"）会让该 bundle **永久无法编辑**。

**附带问题**：`delete_provider_bundle_command` 从未调用 `ensure_ordinary_provider_not_bundle_managed`，
而普通删除路径（`state.rs:4827`）有调用。

**修复方案**：
1. 四处补 `.filter(|stored| is_explicit_bundle_surface(&stored.provider))`
2. 创建 bundle 时若 `(app, id)` 与非 bundle provider 冲突，直接拒绝
3. **治本**：把 `bundle_id` 改为返回 `Option<&str>`，让兜底在类型层面无法被误用

---

### 1.4 🟠 HIGH — H2：`family.google_oauth` 的 bundle 根本无法共享（已实证）

`src/domain/sharing/credential_source.rs:282-297` 是一张**手工维护的字符串白名单**，已与 registry 漂移：

```rust
fn reusable_profile_family(profile_id: &str) -> Option<&'static str> {
    let suffix = profile_id.split_once('.')?.1;
    match suffix {
        "openai_oauth"      => Some("openai_oauth"),
        "grok_oauth"        => Some("grok_oauth"),
        "cursor_oauth"      => ...,
        "antigravity_oauth" => ...,
        "antigravity_cli"   => ...,
        "cursor_api_key"    => ...,
        "ollama_cloud"      => ...,
        "openrouter"        => ...,
        "nvidia"            => ...,
        "deepseek_api"      => ...,
        _ => None,                          // ← google_oauth 与 custom_http 落这里
    }
}
```

`provider-registry.json` 中的多 surface 家族共 12 个：
openai_oauth、**google_oauth**、grok_oauth、ollama_cloud、cursor_oauth、cursor_api_key、
antigravity_oauth、antigravity_cli、openrouter、nvidia、deepseek_api、**custom_http**。

白名单缺 `google_oauth` 与 `custom_http`。随后 `credential_source.rs:142-147` 硬失败：

```rust
if bindings.len() > 1 && candidate.is_none() {
    return Err(CredentialSourceError::ReuseUnsupported { ... });
}
```

**实证**：google_oauth bundle（claude + gemini，共用一个 `g-acct`）创建正常，但

```
建 share → 400 Bad Request
{"error":"Provider claude/g-bundle does not support credential-source reuse",
 "code":"cc_switch_share_credential_source_mismatch"}
```

`google_oauth` 是 ManagedAccount 家族，各 surface 携带相同 `account_id`（`bundle.rs:272-281`），
结构与被允许的 `openai_oauth` / `antigravity_oauth` **完全一致**——纯粹是白名单漏了。

（`custom_http` 的排除可能是有意的：其 `CredentialPolicy::Custom` 确实持有 per-surface 密钥，见 M6。）

**修复方案**：从 registry 推导可复用性
（`family.surfaces.len() > 1 && credential_policy ∈ {ManagedAccount, StaticSecret, Aws}`），
而非字符串后缀匹配；补一条契约测试断言"每个多 surface 家族都可复用"。

---

### 1.5 🟠 HIGH — H3：`refresh_capacity_pool_ids` 在启动路径 fail-closed，坏数据导致进程起不来

```rust
// src/state.rs:3503-3506（启动路径）与 5756-5758（每次 provider 提交）
let refreshed_capacity_pools = shares
    .refresh_capacity_pool_ids(&providers, &accounts, &reasoning_root_key.key)
    .map_err(|error| anyhow::anyhow!("[{}] {error}", error.code()))
    .context("derive Share capacity pools during startup")?;      // ← 任一 share 出错即中止
```

`refresh_capacity_pool_ids`（`shares.rs:738-763`）遍历**所有**未删除 share，首个错误即向上传播。
可达的错误来自**数据**而非用户输入：

| 触发条件 | 错误 | 说明 |
|---|---|---|
| `bindings` 为空 | `InvalidBindingCount`（`credential_source.rs:130-134`） | `bindings` 是 `#[serde(default)]`（`shares.rs:374`），而 `ShareStore::load_or_default`（`shares.rs:494-503`）**不做归一化**，写路径 `invariants.rs:63-70` 才做 |
| 多 binding 但 provider 记录缺失，或 profile 落在 H2 白名单外 | `ReuseUnsupported` | 与 H2 联动放大 |
| 两个 binding 解析到不同账号 | `SourceMismatch` | — |

任一情况**进程直接启动失败**，且无离线修复手段（`admin.rs:168-182` 只能打印 store 摘要）。

这是 `4e56ddd` 引入的**回归**——此前这类 share 只是降级，不会阻断启动。

**修复方案**：
1. 启动路径改为 log-and-skip（保留旧 `capacity_pool_id` 或在 share 上打 `last_error`），
   硬失败只保留在用户可补救的写路径
2. load 时从 `(share.app, share.provider_id)` 回填 `bindings`——
   `ctl.rs:414`、`handlers.rs:1285`、`model_health.rs:336`、`router_contract.rs:591`、
   `shares.rs:2028`、`shares.rs:2057` 已经各自 ad hoc 回填了 6 次，应统一到 load 层

---

### 1.6 🟡 MEDIUM — M4：per-app 的 ACL 与定价被静默塌缩成 share 级

> 本项由两条独立路径分别发现同一根因，导致两个不同后果。

**根因**：`canonicalize_shared_app_settings_for_share`（`shares.rs:2469`）用 share 级值**广播覆盖**所有 app：

```rust
// shares.rs:2493-2512
let price = share.for_sale_official_price_percent_by_app
    .values().next().copied();                       // ← 取 BTreeMap 首个元素

share.access_by_app.clear();
share.app_settings.clear();
share.for_sale_official_price_percent_by_app.clear();

for app in apps {
    share.access_by_app.insert(app.clone(), access.clone());     // 同一份
    share.app_settings.insert(app.clone(), settings.clone());    // 同一份
    if share.for_sale && !share.free_access {
        if let Some(price) = price {
            share.for_sale_official_price_percent_by_app.insert(app, price);
        }
    }
}
```

#### 后果 a — per-app 定价静默丢失

`apply_settings_patch` 在 `shares.rs:1748` 接受完整的 per-app 价格 map：

```rust
if let Some(pricing) = patch.for_sale_official_price_percent_by_app {
    share.for_sale_official_price_percent_by_app = pricing;      // :1748
}
```

**24 行之后**，在 `shares.rs:1772` 调用 canonicalize，按 BTreeMap 字典序首元素
（`claude` < `codex` < `gemini`）广播覆盖。

卖家设置 `{claude: 80%, codex: 50%}` → 实际存储为 `{claude: 80%, codex: 80%}`，
**codex 的 50% 无声丢失**。

这不是无害归一化：Router 的 Token Market **明确按 app 计价**，
`for_sale_official_price_percent_by_app` 是其合同字段。**这是功能回归。**

补充：若 share 为 free（`free_access = true`），整个定价 map 被清空。

#### 后果 b — per-app ACL 被并集放大

`shares.rs:3025-3050` 把所有来源并集：

```rust
share.acl.shared_with_emails.iter()
    .chain(share.access_by_app.values().flat_map(|a| a.shared_with_emails.iter()))
    .chain(share.app_settings.values().flat_map(|s| s.shared_with_emails.iter()))
```

然后 `shares.rs:2996-3021` 为并集中每个 email 创建 **share 级** `user_grant`；
`shares.rs:2987-2993` / `2747-2755` 再把每个 grant email 写回**每个** app 的列表。

结果：`accessByApp: {claude: [alice], codex: [bob]}` → **alice 和 bob 都能调用 claude 和 codex**。

该行为甚至被测试断言为预期（`shares.rs:4328-4332`），因此是**设计 / API 表面不匹配**，而非竞态。

#### 判断与修复

若产品意图就是"一个 share = 一套 ACL + 一个价格"（考虑到共用同一认证源，限额共享是合理的），
那么问题不在行为本身，而在于 **API 与 UI 仍然接受 per-app 输入却静默丢弃**。

当前状态是最坏的一种形态：**接受、不报错、悄悄改变语义。**

两个可选方向：
- **A（收窄）**：在 `normalize_access_by_app`（`shares.rs:3084-3099`）对分叉的 per-app 输入显式报错，
  UI 同步移除 per-app 编辑入口
- **B（真支持）**：让 `ShareUserGrant` 携带 app 集合，并在 `validate_for_invocation` 中校验；
  定价保留 per-app map 不做广播

> 此项需产品决策，且影响 Router Token Market 的计价正确性。

---

### 1.7 🟡 MEDIUM / LOW — 其余问题

| # | 级别 | 问题 | 位置 |
|---|---|---|---|
| **M5** | MEDIUM | **server 本地完全不校验 share ACL**。无 grant 的 email 不会被拒绝，`ShareRejectReason`（`shares.rs:2327-2337`）根本没有"未授权"变体，全权委托远程 share-router。架构上说得通，但 router 的一个 bug 或 `control_secret` 泄漏即直接等价于无限制访问；且未知 email 的 per-user token/parallel 限额被整个跳过 | `shares.rs:1034-1061` |
| **M6** | MEDIUM | `custom_http` bundle 要求**同一把 key 输入 N 次**（3 surface 家族输 3 次），改一个 surface 其余仍是旧 key，**无漂移检测**。直接违背"一套认证源"的特性前提 | `bundle.rs:728-735`；`state.rs:4948-4958` |
| **M7** | MEDIUM | `share_ids_for_provider` 只检查 `bindings`，缺 `bindings.is_empty()` 回退（`shares.rs:2028`、`2057` 有）。早于 `bindings` 引入（`65721b8`，2026-07-07，本特性前约 24 个提交）的 `shares.json` 会报 `shareIds: []`，于是 `provider_reference_preview`（`state.rs:4775-4786`）判定"未被引用"→ 允许删除**正在使用**的 provider。同一 share 因 `shares.rs:982` 空 bindings → 对每个 app 都 `UnsupportedApp`，也永久不可用 | `shares.rs:726-736` |
| **L8** | LOW | `seal_store` 把凭证不一致变成不可恢复的 500：共享凭证 surface 的源无凭证或槽位不一致时 `bail!`。API 层当前由 `validate_profile_credentials`（`state.rs:2482-2497`）挡住，但 `state.rs:2319` 对**禁用**的 bundle surface 跳过该校验。由于 `seal_for_commit` 跑全 store（`state.rs:5747-5749`），一条坏记录会让**所有** provider 编辑失败 | `store_v2.rs:195-210` |
| **L9** | LOW | `validate_provider_bundle_field_scopes` 在 `enabled_surfaces.len() < 2` 时提前返回，禁用的 surface 可持有分叉的 bundle 作用域 endpoint/headers。重新启用时会再校验，影响低，但意味着"bundle 作用域"不是存储数据的不变量 | `state.rs:2026-2032` |
| **L10** | LOW | `accessByApp` 未知 key 处理不对称：upsert 时静默丢弃（`invariants.rs:91` `retain`），import 时报错（`invariants.rs:127-134`）。`app_settings` 同。应统一 | `invariants.rs` |
| **L11** | LOW | **Q8 答案：类型化只做了一半**。`b483969` 确实把 provider 侧类型化了（`AppKind` / `ProfileId` / `DriverId` / `ModelPolicyKind` / `ProviderKey`），但 **share 层未转换**：`access_by_app` / `app_settings` 仍是 `BTreeMap<String, _>`，并在 `router_contract.rs:605`、`672-676`、`841-844` 与 `credential_source.rs:283`（`split_once('.')?.1`）中裸字符串匹配。**H2 就是这个的直接后果** | `shares.rs:362`、`366` |

---

## 2. 技术债

> **前置结论**：这个代码库比它的文件体积看起来健康得多。
> 生产代码 **0 个 `panic!` / `todo!` / `unimplemented!`**，**0 个 `FIXME` / `HACK`**，
> 仅 1 个 TODO（还是测试 fixture 字符串）；原子写 + 双 fsync + 目录锁；真正的 schema 版本化迁移；
> 1,915 个测试。
> 债务集中在**结构与运行时架构**，不是代码卫生问题。**无 Critical 级技术债。**

### 2.1 🔴 唯一可达的死锁：config / ui_settings 锁序倒置

```
src/api/invoke/dispatch.rs:173   let store  = state.ui_settings.read().await;
src/api/invoke/dispatch.rs:174   let config = state.config.read().await;      // ui_settings → config

src/api/invoke/handlers.rs:1782  let config      = state.config.read().await;
src/api/invoke/handlers.rs:1783  let ui_settings = state.ui_settings.read().await;  // config → ui_settings
src/api/invoke/handlers.rs:1874,1876                                // config → shares → ui_settings
```

两侧都是读锁，但 `tokio::sync::RwLock` 是**写优先**的，且 `state.rs:3721-4507` 存在
**20 处 `config.write().await`**。一个写者插入到两次读之间即构成死锁——**可达，非理论**。

**后果**：设置 / dispatch API 路径永久卡死，只能重启进程。

**修复**：把 `dispatch.rs:173` 的顺序改为 `config → ui_settings`。**一行。**
同时把 store 锁层级写进模块级文档（`api/debug.rs:57` 与 `state.rs:3707` 已用显式 `drop(store)` 做对了
——纪律是有的，只是没有被强制）。

> 全局分析 248 个持锁作用域后，store 层级其余部分是一致的：`providers → accounts → usage → shares`。
> 只有上述一对倒置。

### 2.2 🟠 HIGH — 同步 fsync 磁盘 IO 跑在 async runtime 上，且持锁

所有 store 的 `save()` 都是**同步**方法：

- `src/domain/usage/store.rs:351`
- `src/domain/providers/store.rs:231`
- `src/domain/accounts/store.rs:610`
- `src/domain/sharing/shares.rs:505`

最终都落到 `src/infra/storage.rs:95` `write_bytes_atomic_with_hook`，
执行 `file.sync_all()`（`:121`）+ `fs::rename`（`:124`）+ `sync_directory`——**每次 save 两次 fsync**。

`state.rs` 中有 **40 处**这样的调用点位于 async 生产代码里，通常还持着 guard：

```rust
// src/state.rs:7655
let mut usage = self.usage.write().await;
let mut provider_health = usage.provider_health.clone();
let snapshot = provider_health.record(observation);
provider_health.save(&self.config_dir)?;      // 2× fsync，阻塞，写锁仍握着
```

对照：全 crate 仅 **7 处 `spawn_blocking`**、**1 处 `tokio::fs`**。

**后果**：每次 save 占住一个 Tokio worker 达两次 fsync 时长，同时攥着该 store 的写锁，
使代理热路径上所有读该 store 的请求串行化。

**修复**：锁内 clone → drop guard → `spawn_blocking` 保存。`state.rs:5781` 已有正确范例。

### 2.3 🟡 MEDIUM — 结构性债务

| # | 问题 | 数据 | 建议 |
|---|---|---|---|
| **H1** | `forward_with_attempt` **单函数 2,293 行**（`forwarder.rs:1131`），整个请求生命周期（share 绑定、账号 in-flight 获取、执行选择、认证刷新 / 重试、provider 分发、流式处理）内联在一个 body 中。无单测接缝，该文件 11,505 行测试全部得走完整 HTTP fixture | 全仓最长函数 | 按已隐含的阶段拆为 `ForwardPipeline`：`resolve_execution` → `acquire_guards` → `dispatch` → `handle_retry`，各阶段独立单测 |
| **H1b** | `web_invoke_dispatch` **1,918 行**（`dispatch.rs:51`），字符串匹配的命令分发器，**0 测试覆盖** | 全仓最大的零测试文件（2,049 行） | 表驱动测试覆盖每个 arm，并断言 arm 列表与 `web-src/src/types.ts` 一致 |
| **M1** | `ServerStateInner` 是上帝对象（`state.rs:286`）：**6,142 行 impl 块**（3451–9592）、**128 个 public 方法**、**37 个同步原语**（15 `RwLock` + **12 个裸 `AsyncMutex<()>`** + 3 `Mutex` + 7 atomics）。那 12 个哨兵锁（`provider_commits`、`reference_mutations`、`share_lifecycle`、`managed_auth_operations`、`codex_workspace_rebind_transactions`、`client_tunnel_claim`、`setup_flight`、`router_share_sync`、`share_edit_sync` 等）与其保护的数据**没有编译器强制的关联** | 12 条只存在于开发者脑中的锁序约束——正是 §2.1 死锁的成因 | 拆分为 `ProviderState` / `ShareState` / `RouterState` / `AccountState` 门面；把 `AsyncMutex<()>` + `RwLock<T>` 配对合并为 `AsyncMutex<T>`，让锁拥有数据 |
| **M2** | 零测试面集中在 API 层。`src/proxy` 33/33 文件有测试，`src/domain` 覆盖良好；缺口在 `src/api`（`src/api/types` 仅 2/11 有测试） | `dispatch.rs` 2049 / `api/settings.rs` 623 / `providers/matrix.rs` 590 / `api/router.rs` 428 / `api/self_update.rs` 375 / `api/debug.rs` 345 / `control/share_router.rs` 273 / `clients/deepseek/pow.rs` 271 | 部分由 `tests/api_contract.rs`（11,645 行 / 113 个集成测试）缓解，但 dispatcher 仍需单测 |
| **M3** | **418 处 `legacy` 引用，0 个 `#[deprecated]`**（编译器完全不跟踪）。`store.rs`(51KB) 与 `store_v2.rs`(34KB) 双路并存。`registry.rs:836` 硬编码 `legacy_preset_mappings.len() != 29` —— **下次正常修改 registry 就会触发**。`usage/store.rs:28` `legacy_usage_schema_version()` 作为两个结构的 `#[serde(default)]`。`admin.rs:31-83` 仍发布 3 个 CLI 迁移命令 | 集中于 `proxy/provider_ops.rs`(57)、`providers/registry.rs`(42)、`providers/runtime.rs`(33)、`usage/store.rs`(26)、`state.rs`(24) | 把 `!= 29` 换成内容 checksum；为 `store.rs`(v1) 设定移除里程碑（强制迁移版本之后） |
| **M4** | **44 处 `Result<_, String>`** 与 **45 个 `thiserror` 结构化错误类型** + `anyhow`(810 处) 并存。proxy 里的 11 处正好在转发路径上，把状态码与可重试性信息丢在了最需要它的边界 | `src/domain`(19)、`src/proxy`(11)、`src/self_update`(8)、`src/api`(5) | 优先转换 `src/proxy` 的 11 处——`ProxyError` 已携带正确语义 |

### 2.4 🟢 LOW

| # | 问题 | 位置 |
|---|---|---|
| **L1** | 被吞掉的结果：73 处 `let _ =`、320 处 `.ok()`（仅生产代码）。多数合理（best-effort 清理、`file.unlock()`、有界 channel 的 `try_send`）。**真正有问题的 3 处**：`api/router.rs:368` `let _ = sync_share_upsert(...)` 静默丢弃**远程 share 同步失败**，router 与 client 可无声分歧且无日志；`invoke/handlers.rs:780` 改 owner email 时 `let _ = std::fs::remove_file(...)`，删除失败会留下陈旧认证材料；`clients/router/client.rs:2571` | — |
| **L2** | panic 构造近乎为零，但 6 个 `unreachable!` 承载逻辑。生产 `.unwrap()` 仅 27 处（其中 23 处在 `cursor/agent_driver.rs` 的测试辅助代码里）；真正的生产 unwrap 只有 4 处，均为 `event_emitter.rs:750,782,837,871` 的状态机不变量。117 处生产 `.expect()` 基本都在文档化已验证的不变量。**需要关注**：`accounts/managers.rs:472` `ProviderType::CursorOAuth => unreachable!()` ——裸 match arm 无消息，Cursor 账号走到这里会 abort | `managers.rs:472`、`event_emitter.rs` |
| **L3** | 重复传递依赖：`bitflags` v1.3.2 + v2.13.0；**`tower-http` v0.5.2（直接依赖）+ v0.6.11（经 reqwest）** 两份进同一个二进制；`getrandom`/`rand_core` v0.2/v0.6 分裂（经 russh 加密栈）；`thiserror = "1"` 而 v2 已发布。`bitflags` v1 被 `portable-pty → nix v0.25`（2022 年）钉住 | `Cargo.toml` |
| **L4** | 2,227 处生产 `.clone()`（`src/proxy` 597 / `src/api` 473 / `src/domain` 444 / `state.rs` 385）。多数是刻意的 snapshot-then-drop-guard——用分配换持锁时长，是正确取舍。**唯一值得看的**：`event_emitter.rs:782,871` 在 SSE 发射循环内逐事件 clone | — |
| **L5** | 标记债务实际为零（排除 node_modules）：1 个 TODO、0 FIXME、0 HACK、1 处有文档说明的 workaround、4 处中文延后说明（均在前端且都有解释）、10 个窄作用域 `#[allow(dead_code)]` | — |
| **L6** | 配置 / 持久化债务：**核查后基本干净**。原子写有并发写、rename 前失败、无临时文件残留三组测试（`storage.rs:165/197/224`）；`USAGE_SCHEMA_VERSION = 4`、`USAGE_JOURNAL_VERSION = 2`、`IMAGE_STORE_SCHEMA_VERSION` 齐备；`MAX_USAGE_LOGS = 2_000` 有界（`:521`），`usage-logs.jsonl` 是会被压缩截断的 journal 而非无限追加；进程级 `DataDirectoryLock`（`fs2` 咨询锁）。**唯一缺口**：`native_refresh_pending` / `recovery` journal 把损坏收据隔离到 `{stem}-{now_ms}-{suffix}.json`（`state.rs:807`），**无保留上限**——持续失败的刷新会让该目录无界增长 | `state.rs:660-1026` |

---

## 3. UI / UX

### 3.1 技术栈现状

React 18 + Vite 7 + TypeScript，Tailwind 3.4 + shadcn/ui(Radix)，`@tanstack/react-query` v5，
`react-hook-form` + zod，`sonner` toasts，`framer-motion`，`recharts`，`@dnd-kit`。

- **无路由**（`package.json` 无 react-router），导航是 `ServerApp.tsx:34` 的 `useState<View>`
- **设计系统**：`src/server-theme.css` 的 HSL CSS 变量（`:root` / `.dark`），在 `tailwind.config.cjs` 映射；
  外加 5 个手写 CSS 文件（`styles.css` 413 行、`styles/*.css` 396 行）
- **4 个顶层视图**：`providers`(bundles) / `shares` / `settings` / `terminal`
- **最大文件**：`ProviderForm.tsx`(3793)、`ServerProviderForm.tsx`(2485)、`lib/i18n.tsx`(1948)、
  `WebdavSyncSection.tsx`(1867)、`ProviderBundleEditor.tsx`(1641)

---

### 3.2 🔴 一行修复、影响 711 处的样式塌陷（S1）

```css
/* web-src/src/server-theme.css:8,21 */
--foreground:           0 0% 0%;
--muted-foreground:     0 0% 0%;    /* ← 与主文字完全相同 */
--secondary-foreground: 0 0% 0%;
--accent-foreground:    0 0% 0%;

/* :41,54 */
.dark { --foreground: 0 0% 100%; --muted-foreground: 0 0% 100%; }
```

`text-muted-foreground` 在 **145 个文件中被使用 711 次**——提示文字、描述、时间戳、表格元信息，
全部以 100% 对比度渲染，与正文视觉上完全一致。**整个应用没有次级文字层级。**

这也正是大量组件绕过 token 层（见 S2）的原因：语义色阶已经死了。

**修复**：`--muted-foreground: 240 4% 46%`（亮）/ `240 5% 65%`（暗），
`--secondary-foreground` / `--accent-foreground` 同理。**两行，711 个调用点的层级立即恢复。**

---

### 3.3 🔴 交互硬伤（按用户影响排序）

#### U8 — 改密码无确认字段、无 label、不可见 → 管理员自锁

```tsx
// web-src/src/components/settings/ServerSecuritySettings.tsx:107-128
<Input id="server-current-password" type="password"
       placeholder={t("settings.serverSecurity.currentPassword", { defaultValue: "当前密码" })} />
<Input id="server-new-password" type="password"
       placeholder={t("settings.serverSecurity.newPassword", { defaultValue: "新密码" })} />
```

- **仅 placeholder，无 `<label htmlFor>`**（`:108,119` 的 `id` 无人引用），输入后 placeholder 消失，
  复查时无法分辨哪个框是哪个
- **只有一个新密码框，无确认字段**
- 校验只有长度（`:37` `if (trimmedNew.length < 8)`）
- 成功后**立即登出**（`:64` `dispatchEvent(SERVER_AUTH_EXPIRED_EVENT)`）

**一个 typo 就把管理员锁在自己的服务器外面。** 而带 eye toggle 的 `SecretInput` 已存在且用在登录页。

**附带**：管理员密码明文存 `localStorage`（`lib/runtime.ts:129-137` `readCachedPassword`/`writeCachedPassword`），
并预填进表单（`ServerSecuritySettings.tsx:18-20`）。

**修复**：加"确认新密码"字段 + 一致性校验，加真正的 `<Label>`，换用 `SecretInput`。

---

#### U5 — `ConfirmDialog` 的确认按钮永不禁用 → 21 个破坏性操作都可重复点击

```tsx
// web-src/src/components/ConfirmDialog.tsx:99-107
<Button variant={variant === "info" ? "default" : "destructive"}
        onClick={() => onConfirm(checkboxLabel ? checkboxChecked : false)}>
  {confirmText || t("common.confirm")}
</Button>
```

props 接口（`:15-30`）**根本没有 `loading` / `disabled`**。调用方只能改文案绕过：

```tsx
// server/providers/bundles/ProviderBundlesPage.tsx:356-361
confirmText={deletePending ? t("common.loading") : t("common.delete")}
onCancel={() => { if (!deletePending) setDeleting(null); }}
```

pending 期间按钮显示 "Loading…" 但**依然可点**——点三下发三个 `deleteBundle`。
且 `onCancel` 在 pending 时静默 no-op，Escape / 背景点击 / 取消按钮全部无反应、无反馈。

**影响全部 21 个 `ConfirmDialog` 调用点。**

**修复**：加 `confirmDisabled?: boolean`（+ 内联 `Loader2`），并让 `onOpenChange` 显式尊重它。

---

#### U9 — 20 个空 `catch {}` 吞掉 OAuth 账号操作失败

```
components/providers/forms/ClaudeOAuthSection.tsx:127, 136
components/providers/forms/GrokOAuthSection.tsx:98, 107, 142
components/providers/forms/KiroOAuthSection.tsx:111, 120
components/providers/forms/GeminiOAuthSection.tsx:88, 97
components/providers/forms/AntigravityOAuthSection.tsx:92, 101
components/providers/forms/CopilotAuthSection.tsx:129, 138
components/providers/forms/CodexOAuthSection.tsx:239
components/providers/forms/CursorOAuthSection.tsx:99
components/providers/forms/DeepSeekAccountSection.tsx:66, 85
components/providers/forms/ProviderForm.tsx:1310
```

```ts
// ClaudeOAuthSection.tsx:114-137
const handleRemoveAccount = async (accountId, e) => {
  e.stopPropagation(); e.preventDefault();
  try { await removeAccountAndUpdateSelection({ ... }); } catch {}
};
const handleLogout = async () => {
  try { await logoutAccountsAndClearSelection({ ... }); } catch {}
};
```

"移除账号" / "退出所有账号" 服务端失败时：**无 toast、无 console、无状态变化**——行还在原地，
用户以为界面卡死。`GrokOAuthSection.tsx:142` 连**登录**回调提交也吞掉了。

另有 **31 个 handler 只 `console.error` 不给用户任何反馈**，例如
`SettingsPage.tsx:161`（保存设置失败）、`:181`（重启应用失败）、`LogConfigPanel.tsx:51`、
`RectifierConfigPanel.tsx:47,59`、`EnvWarningBanner.tsx:89`（消息还是硬编码中文）。

**修复**：全部替换为 `catch (e) { toast.error(e instanceof Error ? e.message : String(e)) }`；
加 ESLint `no-empty` 规则并设 `allowEmptyCatch: false`。

---

#### U4 — `AuthCenterPanel` 保存无 try/catch、无 pending 态

```ts
// web-src/src/components/settings/AuthCenterPanel.tsx:241-276
const handleSaveQuotaSettings = async () => {
  const { webdavSync: _, ...rest } = settings;
  await settingsApi.save({ ...rest, oauthQuotaRefreshIntervalMinutes: ..., ... });
  await invalidateQuotaQueries();
  toast.success(t("settings.authCenter.quotaSettingsSaved", { ... }));
};
// :404-408
<Button onClick={() => void handleSaveQuotaSettings()}
        disabled={!settings || !hasQuotaSettingChanges}>
```

- 无 `catch`——请求 reject 变成 unhandled promise rejection，成功 toast 不出现，
  **用户看不到任何错误、任何状态变化**
- 无 pending 标志——请求期间 `disabled` 保持 false，每多点一次就多发一个完整 settings PUT

---

#### U7 — 调试 Bearer Token 明文渲染 + 护栏装反 + 复制静默失败

```tsx
// web-src/src/components/settings/ApiManagementPanel.tsx:183-187
{visibleToken ? (
  <div className="flex items-center gap-2">
    <code className="min-w-0 flex-1 break-all border bg-muted/30 p-3 text-xs">
      {visibleToken}
    </code>
```

该 token 可调用 `/web-api/debug/restart` 与 `/web-api/debug/upgrade`（`:61-62`），
却**完全展开渲染、无遮罩、无 reveal toggle、长期停留在屏幕上**——
而项目自己的 `server/ui/SecretInput.tsx`（带 eye toggle）就在 `ProviderBundleEditor.tsx:334,398,1402` 用着。

同文件另外三个问题：

| 位置 | 问题 |
|---|---|
| `:106-117` | **revoke token 无任何确认**，而仅仅"启用"某能力却有 `ConfirmDialog`（`:82-85`）——**护栏装在了错误的动作上** |
| `:119-124` | `copyToken()` 直接调 `navigator.clipboard.writeText`，绕过项目自己的 `lib/clipboard.ts` `copyText()` fallback（`clipboard.ts:5-33`）。**在纯 HTTP 局域网访问（本服务器的常规访问方式）下 `navigator.clipboard` 是 undefined → TypeError，不 `setCopied(true)`，无错误 toast，完全静默失败**。同样问题另见 `SessionManagerPage.tsx:395`、`LocalEnvCheckSettings.tsx:219` |
| `:226-242` | 日志行数 `<Input type="number">` 在 `onChange` 里 `void save({...})`——**每敲一个键就发一次 settings PUT** |
| `:126` | `if (loading) return null;` ——加载期间整个面板什么都不渲染，然后突然弹出 |

---

#### U6 — 两个最大的编辑器在取消时丢弃全部输入且无提示

`hooks/useUnsavedChangesGuard.ts` **存在**且已接 `beforeunload`，但只被 **2 个文件** import：
`AddProviderDialog.tsx:8` 与 `EditProviderDialog.tsx:8`（均继承自桌面版）。

**未受保护的**：

- `server/providers/bundles/ProviderBundleEditor.tsx:1622-1629` —— `onClick={onCancel} disabled={saving}`；
  该 1641 行文件 `grep -c dirty` = **0**。它编辑 API key、endpoint、header、超时和完整 share 配置。
  **点一下取消就全没了。**
- `components/settings/SettingsPage.tsx` —— `grep -n "isDirty\|dirty"` 无结果。
  Advanced 页用显式保存按钮（`:631-652`）而 General 页自动保存（`:192-221`），
  因此切 tab、点 header 返回箭头（`ServerApp.tsx:198-205`）或 `SettingsPage.tsx:97` 的 `onOpenChange`
  都会**无提示丢弃 Advanced 的未保存修改**。

---

### 3.4 🔴 i18n：英 / 日用户会看到 75 个中文字符串（U1）

`t(key, { defaultValue: "中文" })` 有 **768 处**，另有 142 处位置参数式 `t(key, "中文")`。
逐个对照 `i18n/locales/en.json` + `i18n/server-locales/en.json`（`fallbackLng: "en"`，`i18n/index.ts:148`）后，
**75 个 key 在任何 locale 文件中都不存在**，兜底兜不住，直接渲染中文 defaultValue：

| 文件 | 泄漏数 |
|---|---|
| `server/providers/bundles/ProviderBundleEditor.tsx` | **17** |
| `components/providers/forms/KiroOAuthSection.tsx` | 16 |
| `components/providers/forms/AntigravityOAuthSection.tsx` | 14 |
| `components/providers/forms/GeminiOAuthSection.tsx` | 14 |
| `components/providers/forms/GrokOAuthSection.tsx` | 13 |
| `server/providers/bundles/ProviderBundleCard.tsx` | 1 |

`ProviderBundleEditor` 是 server UI **最核心的编辑界面**。英文用户看到的是：

```
ProviderBundleEditor.tsx:306  providerBundle.queryParameter  => "API Key 查询参数名"
ProviderBundleEditor.tsx:461  providerBundle.requestTimeout  => "请求超时（毫秒）"
ProviderBundleEditor.tsx:780  provider.share.forSale         => "访问模式"
ProviderBundleEditor.tsx:798  provider.share.private         => "私有"
ProviderBundleEditor.tsx:873  provider.share.tokenLimit      => "Token 限额"
ProviderBundleEditor.tsx:959  provider.share.expiry          => "有效期"
```

四个 OAuth 区块泄漏了整套账号管理词汇：
`notAuthenticated → 未认证`、`setAsDefault → 设为默认`、`removeAccount → 移除账号`、
`logoutAll → 退出所有账号`、`openLinkHint → 授权链接不会自动打开，请点击或复制后在浏览器中访问：`

**修复**：把 75 个 key 补进 `en.json` / `ja.json`；加 CI 检查——
`defaultValue` 含 CJK 且 key 不在 `en.json` 中则构建失败。

#### U2 — zh-TW 缺 401 个 key，静默回退英文

key 数量：`zh` 3003 / `en` 2999 / `ja` 2940 / **`zh-TW` 2618** → **zh-TW 缺 401 个**，
包括 `common.commit`、`common.disabled`，以及**整个 `provider.share.*` 段落**
（`enableShare`、`sharing`、`stop`、`resume`、`delete`、`deleteFailed`、`saveSuccess`）。
繁体中文用户在 share 界面看到的是中英混排。

另外 `i18n/index.ts:15` 与 `lib/i18n.tsx:33` 都设 `DEFAULT_LANGUAGE = "zh"`——
浏览器语言为 fr / de / ko 的访客经 `getInitialLanguage()`（`i18n/index.ts:40-65`）
落到**简体中文**而非英文。

**修复**：补齐 zh-TW（或 `fallbackLng: { "zh-TW": ["zh", "en"], default: ["en"] }`）；
`DEFAULT_LANGUAGE` 改为 `"en"`。

#### U3 — 两套并行的 i18n 系统

`lib/i18n.tsx`（1948 行）是第二套翻译引擎，自带内联 `serverResources` 树覆盖
`server.auth.*` / `server.nav.*` / `common.*` 四种语言（`:42-780`），
**这些 key 在 `i18n/server-locales/*.json` 中完全不存在**。
17 个文件用 `useI18n`，139 个用 `useTranslation`，两边各自维护 `common.cancel` / `common.confirm`。

更严重的是 `lib/i18n.tsx:1858-1879` 的 `composeText()` 做**逐词机器翻译**：

```ts
const translated = text.split(/(\s+|\/|-)/)
  .map((part) => { ... return words[part] ?? words[toTitleCase(part)] ?? part; })
  .join("").replace(/\s+/g, language === "ja" ? "" : " ");
```

按空格切开查词典再拼回去，**无语法处理**。任何传给 `tx()` 的新英文标签
都会产出没有任何译者见过或审过的破碎中 / 日文。

**修复**：把 17 个 `useI18n` 文件迁到 `react-i18next`，`serverResources` 移入 `server-locales/*.json`，
删除 `composeText` / `tx`。

---

### 3.5 🟡 其他样式问题

| # | 问题 | 位置 |
|---|---|---|
| **S2** | shadcn 基础组件硬编码 `blue-500` / `gray-*` 而非 `--primary` / `--muted`，**产生两种不同的蓝**：`blue-500` = `#0A84FF`（`tailwind.config.cjs:65`），`--primary` = `210 100% 56%` ≈ `#1F8FFF`，两者在每个界面并排出现。`outline` / `ghost` 的 hover 用被覆盖的 gray 色阶，完全不跟随主题 | `ui/button.tsx:12-25`、`ui/tabs.tsx:29` |
| **S3** | **Web 终端硬编码 GitHub-Light 配色**（`background: "#f6f8fa"`），另 `:382` 有 `bg-[#f6f8fa]`。终端是四个顶层视图之一，暗色模式下是深色外壳里的整屏刺眼白板。`hooks/useDarkMode.ts` 已存在但全仓只用了一次 | `TerminalPage.tsx:139-160`、`:382` |
| **S4** | 4 个骨架屏用 `bg-gray-100` **无 `dark:` 变体**，暗色下用量看板每次加载闪 4 个白块；且固定 `h-[400px]` 与实际内容无关，数据到达时布局跳动 | `ModelStatsTable.tsx:38`、`ProviderStatsTable.tsx:38`、`RequestDetailPanel.tsx:39`、`RequestLogTable.tsx:146` |
| **S5** | 手写 CSS 状态徽章**无 `.dark` 覆盖**（`grep -c '\.dark'` 在每个 CSS 文件都返回 0）。背景是 ~95% 亮度字面量，而 `--success` 暗色下变 `151 58% 40%` → 薄荷绿叠薄荷绿，几乎不可读 | `styles.css:354-356`、`styles/auth-accounts.css:7-9,94-97` |
| **S6** | **全局隐藏滚动条**：`* { scrollbar-width: none }` + `::-webkit-scrollbar { display: none }`。这是 Tauri 桌面端习惯泄漏到**浏览器**管理控制台——设置面板（`SettingsPage.tsx:266-269`）、provider 列表、终端、用量表格滚动时**零视觉提示、无可拖拽滑块**，长的 Advanced 页用户根本不知道下面还有内容 | `server-theme.css:135-139,157-160` |
| **S7** | 焦点环设了宽度和颜色但**没设 `outline-style`**：`@apply outline-2 outline-blue-500 outline-offset-2` 只产出 `outline-width` / `outline-color`，Chrome 保留 UA 的 `outline-style: auto` 并**忽略宽度和颜色**——设计的 2px 品牌焦点环从未真正生效，且各浏览器表现不一。用的还是 `blue-500` 不是 `--ring` | `server-theme.css:162-164` |
| **S8** | **约 10% 组件目录是死代码**：`SimpleModal.tsx`、`ModalFooter.tsx`、`KeyValue.tsx`、`TextField.tsx`、`LoadingBlock.tsx`、`JsonPreview.tsx`、`StatusPill.tsx`、`DatabaseUpgrade.tsx`(300 行)、`FirstRunNoticeDialog.tsx`、`DeepLinkImportDialog.tsx`(659 行) 引用数均为 0；`src/index.css`(236 行) 从未被 import；`src/styles/modals.css` 只被死掉的 `SimpleModal` 使用。`SimpleModal.tsx:25-27` 是手搓的第二套模态实现——无 `role="dialog"`、无 `aria-modal`、无 Escape、无焦点陷阱。**今天是死的，但明天有人会照着抄** | — |
| **S9** | 77 处小于 12px 的任意字号（`text-[10px]`×57、`text-[11px]`×18、`text-[9px]`×2），色阶最小值是 `text-xs`(12px)——数据密集控制台里 9–10px 正文低于可读下限。**6 种圆角并存**（`rounded-lg` 123 / `rounded-md` 115 / `rounded-full` 63 / 裸 `rounded` 60 / `rounded-xl` 56 / `rounded-sm` 10 / `rounded-2xl` 3），无使用规则。**主 CTA 用了 token 集里不存在的颜色**：`ProviderBundlesPage.tsx:272` 的 "Add provider" 是 `bg-orange-500 shadow-orange-500/30` | — |
| **S10** | `ProviderBundlesPage.tsx:238-245` 用 `hidden ... sm:block`，**640px 以下标题完全消失**，只剩 5 个无标签图标按钮且无页面标识；`SettingsPage.tsx:588,607,634` 与 `FullScreenPanel.tsx:142` 重复内联 `style={{ backgroundColor: "hsl(var(--background))" }}` 而非 `bg-background`；`ServerSecuritySettings.tsx:122` 同一元素上有 `sm:w-44 sm:w-52` 两个冲突宽度 | — |

---

### 3.6 🟡 其他交互问题

| # | 问题 | 位置 |
|---|---|---|
| **U10** | `SharePage` 每 60s 轮询时都 `setLoading(true)`（`ShareOwnerAuthBar` 每分钟闪一次、布局跳动）；且**任何瞬时失败都 `setSession({ authenticated: false })`**——一秒的网络抖动会让 UI 声称 owner **已从 Router 登出**（最惊悚的错误状态），真实错误只进 console | `SharePage.tsx:323-346` |
| **U11** | **全仓唯一一个 `window.confirm()`** ——未加样式、不可主题化、阻塞线程的 OS 对话框，出现在主题化控制台中间；某些浏览器可整体抑制它，届时 reset-usage 会静默执行。而两个元素之外的删除按钮用的是 `<ConfirmDialog>` | `ShareCard.tsx:282-287` |
| **U12** | `staleTime: 0` + `refetchOnWindowFocus: true`——每个已挂载 query 立即过期，切回标签页时**全部同时重发**（providers 视图 3 个，usage 视图 6 个以上）。叠加各面板定时器：`SharePage.tsx:341`(60s)、`UsageDashboard.tsx:68`(30s)、以及 5 个配额页脚的 `setInterval(…, 30000)`（`SubscriptionQuotaFooter.tsx:374`、`CopilotQuotaFooter.tsx:124`、`CursorOauthQuotaFooter.tsx:118`、`KiroOauthQuotaFooter.tsx:103`、`OllamaQuotaFooter.tsx:113`） | `lib/query/queryClient.ts:5-20` |
| **U13** | **无路由**：视图存 `useState` + `localStorage`（`ServerApp.tsx:34,60-68`），编辑器靠 `if (editing) return <ProviderBundleEditor/>` 整页替换。后果：浏览器返回键**直接退出应用**而非退出编辑器；刷新丢失半填表单；无法把链接发给同事；`SettingsPage.tsx:119-131` 被迫手搓 tab 别名映射，只因没有 URL | — |
| **U14** | `FullScreenPanel` 是手搓模态：Escape 处理有（`:64-88`），但**无 `role="dialog"`、无 `aria-modal`、无焦点陷阱、无焦点还原**——底层页面仍在 tab 顺序里，Tab 会直接走出面板到后面看不见的控件上 | `FullScreenPanel.tsx:133-143` |
| **U15** | 全量渲染与未保护的异步：`ModelStatsTable.tsx:72` / `ProviderStatsTable.tsx` 无分页无上限无虚拟化（`@tanstack/react-virtual` 是依赖但只用在 `SessionManagerPage.tsx:4` 一处；`RequestLogTable.tsx:58` 倒是有 `pageSize = 20`）；`ProviderBundlesPage.tsx:326-343` 在 `DndContext` 里渲染全部 bundle 不虚拟化；`ProviderBundleEditor.tsx:1126-1132` 的 `shareApi.suggestShareSlug().then(...)` **无 `.catch`**，失败则子域名框永远空着且无解释 | — |
| **U16** | Toast 时长不一致：`ui/sonner.tsx:17` 全局 `duration: 2000`，调用方却覆盖了 28 次（`0`×14、`3000`×7、`5000`×2、`4000`×2、`6000`×1、`10000`×1）。2 秒默认值低于错误消息（含服务端异常串）约 4 秒的可读下限，而错误与成功不应共用时长 | — |

---

### 3.7 前端 Top 8（按用户影响）

| # | 问题 | 位置 | 成本 |
|---|---|---|---|
| 1 | `--muted-foreground` == `--foreground`，711 个调用点失去文字层级 | `server-theme.css:21,54` | 2 行 |
| 2 | 75 个中文串展示给 EN/JA 用户，含整个 provider 编辑器 | `ProviderBundleEditor.tsx` + 5 文件 | locale 回填 + CI 检查 |
| 3 | 改密码无确认字段 / 无 label / 不可见 → 管理员自锁 | `ServerSecuritySettings.tsx:107-128` | 小 |
| 4 | `ConfirmDialog` 确认按钮永不禁用 → 21 个破坏性流程可重复触发 | `ConfirmDialog.tsx:99-107` | 小，单文件 |
| 5 | 20 个空 `catch {}` 吞掉 OAuth 账号移除/登出失败 | 10 个 `*OAuthSection.tsx` | 机械性 |
| 6 | `AuthCenterPanel` 保存无 try/catch 无 pending，用户无法得知结果 | `AuthCenterPanel.tsx:241-276,404-408` | 小 |
| 7 | Bearer token 明文渲染；revoke 无确认；HTTP 下复制静默失败 | `ApiManagementPanel.tsx:106-124,183-187` | 小 |
| 8 | `ProviderBundleEditor` + Settings 取消/返回时丢弃未保存内容 | `ProviderBundleEditor.tsx:1622`、`SettingsPage.tsx:97` | 中 |

---

## 4. 建议的动手顺序

| 序 | 动作 | 成本 | 理由 |
|---|---|---|---|
| **0** | **确认 working tree 中 `tunnel.rs` / `state.rs` 两个未提交文件的来源** | — | 不确认就动手会与另一路工作互相踩踏 |
| **1** | **C1**：`state.rs` 的 4662 / 4713 / 4717 / 4726 / 5034 五处补 `is_explicit_bundle_surface` 过滤；bundle 创建时拒绝 `(app, id)` 冲突；`bundle_id` 改返回 `Option<&str>` | 小时级 | 唯一会**静默销毁用户凭证**的缺陷 |
| **2** | **死锁**：`dispatch.rs:173` 一行换序为 `config → ui_settings`，并把 store 锁层级写进模块文档 | 分钟级 | 消除一整类进程永久卡死 |
| **3** | **H3**：`refresh_capacity_pool_ids` 启动路径改 log-and-skip；`ShareStore::load_or_default` 中统一回填 `bindings` | 小时级 | 消除"坏数据 = 起不来"，顺带收敛 6 处 ad hoc 回填 |
| **4** | **H2**：可复用性改为从 registry 推导 + 契约测试断言每个多 surface 家族可复用 | 小时级 | `google_oauth` 家族当前完全不可用 |
| **5** | **U8 + U5 + U9**：改密码确认字段、`ConfirmDialog` 禁用态、清空 20 个 `catch {}` | 天级 | 三者均属"用户以为成功了其实没有"或"把自己锁在门外" |
| **6** | **S1 两行**（`--muted-foreground`）+ 回填 75 个 i18n key + 加 CI 检查 | 天级 | 投入产出比最高的观感修复 |
| **7** | **M4 决策**：per-app ACL / 定价——要么真支持（方向 B），要么 API 层显式拒绝（方向 A） | 需产品决策 | 当前"接受输入、静默改语义"是最坏形态；且影响 Router Token Market 的按 app 计价正确性 |
| **8** | **技术债 §2.2**：40 处 `save()` 改为 clone → drop guard → `spawn_blocking` | 天级 | 代理热路径最大的延迟来源 |
| **9** | `dispatch.rs` 表驱动测试 → 再拆 `forward_with_attempt`(2293 行) 与 `ServerStateInner`(6142 行 impl) | 周级 | **必须先有测试网**，顺序不可颠倒 |

---

## 附：问题速查索引

### 后端缺陷

| ID | 级别 | 一句话 | 主位置 |
|---|---|---|---|
| C1 | CRITICAL | 删 bundle 连带删同名无关 provider（含凭证） | `bundle.rs:675`、`state.rs:4713` |
| H2 | HIGH | `google_oauth` bundle 无法建 share（白名单漂移） | `credential_source.rs:282-297` |
| H3 | HIGH | 坏 share 数据导致进程启动失败，无离线修复 | `state.rs:3503`、`shares.rs:738` |
| M4a | MEDIUM | per-app 定价被广播覆盖，静默丢失 | `shares.rs:1748` → `1772` → `2493` |
| M4b | MEDIUM | per-app ACL 被并集放大为 share 级 | `shares.rs:3025-3050` |
| M5 | MEDIUM | server 本地不校验 share ACL，全权委托 router | `shares.rs:1034-1061` |
| M6 | MEDIUM | `custom_http` bundle 需重复输入同一 key，无漂移检测 | `bundle.rs:728-735` |
| M7 | MEDIUM | 老 `shares.json` 使 provider 引用检查失效，可删除在用 provider | `shares.rs:726-736` |
| L8 | LOW | `seal_store` 把凭证不一致变成全局 500 | `store_v2.rs:195-210` |
| L9 | LOW | 禁用 surface 可持有分叉的 bundle 作用域配置 | `state.rs:2026-2032` |
| L10 | LOW | `accessByApp` 未知 key 在 upsert/import 间处理不对称 | `invariants.rs:91` vs `127` |
| L11 | LOW | share 层仍是 stringly-typed（H2 的根因） | `shares.rs:362,366` |

### 技术债

| ID | 级别 | 一句话 | 主位置 |
|---|---|---|---|
| — | HIGH | config/ui_settings 锁序倒置，可达死锁 | `dispatch.rs:173` vs `handlers.rs:1782` |
| — | HIGH | 40 处同步双 fsync 保存跑在 async 上且持锁 | `state.rs`（40 处）、`infra/storage.rs:95` |
| H1 | HIGH | `forward_with_attempt` 单函数 2293 行 | `forwarder.rs:1131` |
| H1b | HIGH | `web_invoke_dispatch` 1918 行且 0 测试 | `dispatch.rs:51` |
| M1 | MEDIUM | `ServerStateInner` 上帝对象，37 个同步原语 | `state.rs:286` |
| M2 | MEDIUM | 零测试面集中在 `src/api` | 见 §2.3 |
| M3 | MEDIUM | 418 处 legacy / 0 个 `#[deprecated]`；`!= 29` 硬编码断言 | `registry.rs:836` |
| M4 | MEDIUM | 44 处 `Result<_, String>` 与 45 个结构化错误类型并存 | 见 §2.3 |

### 前端

见 §3.7 Top 8 与 §3.5 / §3.6 表格。
