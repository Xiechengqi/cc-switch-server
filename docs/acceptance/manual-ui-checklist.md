# Manual UI Checklist

This checklist is the manual UI acceptance gate for cc-switch-server.
Do not replace it with Playwright, Cypress, Puppeteer, Selenium, browser screenshot
scripts, or automated click flows.

## Scope

- Validate the Web UI against Server product requirements, Server APIs, and `assets/contract/web-runtime-contract.json`.
- Retained pages: Providers, Shares, Usage, Settings/Auth/Router/Backup, Accounts/OAuth/quota.
- Excluded from server (must not appear): Universal Providers, import-current-CLI-config, skills, MCP, OpenClaw workspace/tools/agents, Hermes, OMO, Tauri shell, updater, deeplink, Claude Desktop profile writing, WebDAV/S3 sync, speedtest, local CLI session parsing, `codex_responses_ws`.

## Viewports

Run the checks manually at:

- Wide viewport: 1366x768 or wider.
- Narrow width: around 390px wide.

## Global

- Shell renders without blank first screen after setup/login state is known.
- Navigation labels do not wrap into unreadable text or overlap icons.
- Topbar actions remain clickable and do not cover page content.
- Tables scroll horizontally when needed instead of overflowing the viewport.
- Buttons and inputs keep stable dimensions while loading or changing state.
- Input and textarea placeholder text stays visibly lighter than entered values in both themes.
- No server-only hidden/excluded feature is visible.
- No browser console or network error is ignored during manual inspection.

## Providers

- New Custom Bundles default to `Custom Provider` and a neutral sliders icon in both the Family picker and editor. Enabled Apps and authentication defaults come from the Registry. Missing Custom icons use the neutral display fallback; existing explicit names and icons are preserved.
- Neither Custom creation nor editing exposes a Quick preset control, including in advanced settings. Configure enabled APIs, upstream protocol, authentication and model policy through their dedicated fields; there is no template action that silently disables other APIs or changes model-policy scope. Existing Anthropic Bearer relays keep their names, icons, enabled APIs, bindings, stored credentials and model policies when opened or saved with an unrelated edit. Bearer authentication remains explicitly configurable in advanced compatibility settings; retained Registry recipes are compatibility/coverage evidence, not UI actions.
- New Custom Surfaces show API URL, upstream protocol and API Key without requiring a choice of authentication headers. Advanced compatibility starts collapsed; selecting another protocol updates recommended authentication from the Registry, independent of the client App. Only Driver-supported combinations appear in advanced settings, and URL-parameter authentication carries a key-exposure warning.
- Existing Custom Bearer relays keep their stored authentication when opened, when advanced settings are expanded, and when unrelated fields are saved. Choosing protocol recommendations is explicit and any actual binding change still requires the rebind preview/confirmation. Saved bindings stay pinned across restart; no authentication probing or automatic inference retry occurs.
- Saving a Custom draft first validates the unsaved endpoints, credential patches and sharing intent. A mismatch identifies the affected App and opens its relevant field/advanced settings without writing a partial Provider. Authentication or tenant conflicts never show the generic refresh message reserved for actual revision/preview expiry. The editor is disabled while saving.
- Protocol-only rebinds with unchanged credential source preserve capacity pools; Provider saves/rebinds preserve Share URLs, grants, Token/USD-equivalent usage, pending quota policies and manual corrections. No production upstream or Router acceptance is implied by a local validation success.

- The page lists Provider Bundles rather than separate per-App Provider records. Each Bundle title shows the logos for every supported Claude, Codex, and Gemini Surface.
- Create a Custom Bundle with only Codex enabled. After saving, change Responses to Chat or change authentication, save, and confirm the binding summary. Reopen and verify the protocol, auth field, endpoint, and retained credential; disabled Claude/Gemini must not require credentials. Cancelling confirmation leaves all server data unchanged.
- If Share saving fails after Provider saving succeeds, the page must say which stage failed, retain the Share draft, and allow retry without writing the Provider again. A lost Provider response must trigger a read-back and preserve entered values; it must not silently report success or automatically repeat credential writes.
- Add Provider starts with one Family selector sourced from the Server Provider registry. Selecting a Family automatically creates its complete authoritative Surface set; the operator never adds or removes arbitrary App records.
- New Provider creation exposes three ordered steps: Choose type, Configure, and Remote share. Future steps stay truly disabled until the operator advances with Next; previously reached steps remain available as shortcuts, and draft values survive backward navigation.
- The Family selector has exactly three product categories in fixed order: Custom, Subscription accounts, and API Key. Custom is pinned first; because its single card already carries the category name, no heading is printed above it and its auth badge is suppressed, so the word appears exactly once at the top of the picker. The explicit subscription allowlist (including Cursor API Key, Ollama API Key, and Kimi API Key) is honored, and any future Family not on the allowlist defaults to API Key.
- Every visible, creatable Family appears exactly once, including Custom, and the matrix matches the provider coverage audit. OpenCode, OpenClaw, Hermes, Claude Desktop, Universal Providers, raw env/TOML editors, automatic failover, and outbound proxy controls never appear.
- Selecting Grok OAuth automatically shows Claude, Codex, and Gemini as icon-labelled tabs. Switching tabs does not discard unsaved values or change Bundle-wide fields.
- Bundle name, Family identity, OAuth/managed account, shared credentials, common endpoint, shared driver options, test configuration, Provider-level timeouts, and Remote Share controls remain outside the Surface tabs.
- Model policy control defaults to Global below credentials and outside the Surface tabs. Global applies one policy to every configurable Surface; fixed Profiles remain read-only exceptions and are identified in the global summary. Per-App control is offered only when at least two Surfaces are configurable.
- Surface tabs never contain a model policy. In Per-App mode each configurable App's policy and upstream model appear as one row per App inside the Model card, next to the read-only rows for fixed Profiles. A Surface tab contains only that App's enable state, custom endpoint override when the Family permits it, protocol/auth configuration when custom binding permits it, Surface credential slots and headers, and the read-only effective runtime diagnostic.
- Automatic health-check App is read-only and always follows Codex → Claude → Gemini among enabled, probe-capable Surfaces. No manual selector appears; an untestable Bundle is labelled unsupported. The model resolves as Surface override → that App's Provider override → that App's Server default. Provider and Surface override inputs stay blank when inheriting and show the inherited value as a light placeholder that disappears on focus.
- Opening a legacy Bundle configured to test Claude while Codex is enabled displays Codex without applying Claude's model override to it. Disabling Codex changes the read-only App to Claude, then Gemini if appropriate. A failing or rate-limited selected App does not silently select a different API. Saving and reopening keep the fixed automatic policy.
- Request, first-byte, and stream-idle timeouts resolve field by field as Provider override → Server default. All three are entered and displayed in seconds. They appear once at Bundle scope, never in Surface tabs; blank inputs preserve inheritance and show the Server values as light placeholders that disappear on focus.
- No editable raw Provider JSON, env object, TOML, `settingsConfig`, `meta`, cache/thinking/governance switch, image model, or video model appears. Saved Surfaces may expose the secret-free compiled Runtime Plan under a collapsed, read-only Effective Runtime Configuration diagnostic with a copy action.
- Custom Header and Query authentication require an explicit header or query-parameter name. Custom Header names, extra Header names, URL fields, timeout ranges, and test-model length reject invalid values before save and are revalidated by the Server.
- Managed Families require one explicitly selected compatible Account shared by their enabled Surfaces. Account and quota refresh updates do not mark unrelated Bundle fields dirty.
- Static secrets use authenticated slot-scoped reveal and password controls. Opening or revealing a secret does not dirty the draft; edits submit `replace`, an unchanged secret submits `keep`, and clearing an optional secret submits `clear`. List and detail responses remain redacted.
- Disabling a Surface removes it from the Bundle's enabled App set. That Surface requires no credential, is absent from the active runtime index and routes, and is excluded from Bundle Share bindings; a read-only compile diagnostic may still be returned for editing.
- Fixed Family/Profile identity, canonical website, endpoint, protocol, and auth fields cannot be changed through display-name or metadata edits. The Custom Family exposes only the explicit custom controls allowed by the registry.
- The editor has exactly one global Save action at the bottom. There are no per-tab Save buttons, and saving submits the complete Bundle plus the one Bundle-scoped Share configuration.
- Create/edit/test/fetch-model actions follow the capability gates of the active Surface without introducing a current/selected Provider state.
- Bundle cards and the editor expose no Switch, Select, Set Current, Clear Current, or hot-switch action. Runtime routing is resolved from the request route, Bundle Surface, or Share binding.
- Bundle cards show the explicit Global or Per-App scope and each enabled App's effective model policy. A Global summary is compacted once, while fixed Profile exceptions remain labelled separately and long model names truncate without resizing actions.
- Saving, reloading, editing, and deleting a Bundle preserve its complete Surface set and revision. A Bundle referenced by a Share cannot be deleted until the reference is removed.

## Shares

- Share status, owner, tunnel/subdomain, provider binding, private/public-free access, authorized users and quotas, Router-managed grants, pending edits, and connect info are visible.
- One Provider Bundle maps to at most one Share. All enabled Claude, Codex, and Gemini Surface bindings use that same Share record, subdomain, and Share URL; no per-App Share URL is created.
- The Bundle editor keeps Remote Share outside the Surface tabs and uses the same bottom Save action. The Server derives bindings from enabled Surfaces instead of trusting App/provider binding fields from the browser.
- Custom multi-App sharing runs a Server-owned, read-only credential-source preflight. Matching primary keys, service/tenant scopes and authentication carriers permit one Share; mismatched keys, hosts, tenant paths, carriers or extra authentication headers must show a localized reason without revealing credentials. A disabled, unconfigured Surface does not block sharing. Rejection retains the draft and creates no Share.
- Changing one enabled Custom Surface's key to a different value while sharing is rejected atomically. Changing all enabled Surfaces together, or an explicit protocol/auth rebind that retains the existing Share binding types and a matching credential source, preserves the Share URL, grants, limits and consumed usage. A rebind that changes a referenced compatibility Provider type is rejected without writes; explicit rebind must not silently replace a Share binding. Restart retains the valid Share and its derived capacity pool. These are manual acceptance items, not a claim of live upstream or deployed acceptance.
- Enabling or disabling a Bundle Surface and saving reconciles that one Share's bindings without changing its URL, access policy, limits, grants, or tunnel identity.
- Share Owner is read-only and always displays Client Owner; Provider Share create/save requests do not submit an independent owner. Changing Client Owner through verified email ownership updates every Share and preserves a valid previous owner as shared access.
- A new Share is private by default. Provider quick-enable and Provider Bundle creation never enable public access implicitly.
- The only public-access control is the “Public free access / 公开免费使用” checkbox. Disabled means private; enabled means any signed-in Router user can invoke for free. The UI has no For Sale, Market access mode, authorized-email text box, Token Market selector, or official-price-percentage input.
- “Authorized users and quotas” is the only manual ShareTo editor. Adding/removing a user updates `userGrants`; no second email list can diverge. In public-free mode these entries remain individual quota overrides rather than an exclusive allowlist.
- Router Share Market-managed grants are labelled and read-only. Saving ordinary Share settings preserves them even when other authorized users are added, edited, batch-edited, or removed.
- Switching to public-free while the Router reports an active Share Market listing/subscription fails visibly and leaves the Share private; after all market entitlements are released, the same save can succeed.
- Share cards and read-only views summarize access as Private or Public free and derive the authorized-user list from active `userGrants`, not legacy ACL projections.
- Pause/resume/binding/tunnel actions are disabled or gated consistently with server state.
- Share connect info can be inspected without exposing excluded client-only features.
- Automatic Share monitoring selects one probe-capable App from the Share's own enabled bindings in fixed Codex → Claude → Gemini order. A Claude-only Share does not cause an extra unused Codex Bundle probe. Router-driven monitoring and the local fallback reuse an identical runtime's half-hour observation; historical other-App records remain history. Manual per-App connection tests remain available and do not change automatic selection. Local tests do not imply deployed Router or upstream acceptance.
- The full Shares page scrolls vertically to the bottom at both target viewports; expanding settings or request logs does not leave content clipped below the shell.
- Request logs show the selected Share's recent seven-day history with correct token, status, latency, range, and pagination values; the table remains horizontally scrollable on narrow screens.
- After a server restart, requests written since the last usage snapshot still appear, and a completed streaming request keeps its final token and latency values.
- User Token periods show Lifetime, Daily, UTC calendar week, Every 7 days, Calendar month, and Every 30 days. The two fixed periods require a non-future UTC start time, preserve minute precision, and hide/clear that field for all calendar periods.
- Two Share users with the same fixed period but different starts show independent current windows and reset countdowns. Changing a user's period or start recomputes the current window from request history instead of resetting usage to zero.
- Editing one user's quota shows the current-period consumed Token count, the current effective baseline, and the UTC window. Saving an explicit `0` is distinct from leaving the field blank; blank leaves the Server baseline unchanged, while “Clear baseline” removes it.
- A consumed-token edit accepts a past fixed-period start (including the previous 7/30-day phase), rejects a target below observed history, and after the save the next request is added to the saved target rather than replacing it. Router Share Market-managed grants remain read-only.
- The consumed-token dialog reads the effective value, the observed history floor, the UTC window, and the standing manual correction from the Server-returned `usageQuota` view. The browser never re-derives them from `usageRebase`, so a Server-side formula change cannot leave the dialog quietly disagreeing with the save it is about to submit.
- The Share settings form exposes a Share-total consumed-Token field separately from the per-user quota editor. It appears only for an existing Share, submits only when the operator actually edits it, and leaving it untouched never overwrites requests that landed while the form was open.
- Setting the Share total below its Token limit clears the `exhausted` state to `paused`; the Share stays disabled until the operator resumes it explicitly. Setting it at or above the limit re-exhausts and disables the Share. Clearing sets zero.
- The Share total is a plain accumulator: unlike a per-user quota it is never rebuilt from Usage history, so an edit is a direct set and later requests continue accumulating on top of it.
- Provider official quota (upstream subscription/rate-limit state shown on Accounts/OAuth) stays display-only. It is external upstream data the Server cannot rebase; the only supported action is refreshing it from the upstream, never editing it locally.
- Reset usage on a Share clears both the derived per-user snapshots and their operator baselines. After a reset no previously saved baseline reappears on the next request or reload.

## Usage

- Overview cards and trends use the same half-open time range and global filters as every table. Request count includes only user inference; supplemental calls add Token volume without adding requests; health probes stay absent.
- The four tabs are Requests, Providers, Models, and Share / Users. Provider rows aggregate a Bundle once and show its Claude/Codex/Gemini Surface breakdown; models are grouped by `(Surface, actual upstream model)`.
- Filters for Claude/Codex/Gemini Surface, Provider Bundle, Share, normalized user email, `(Surface, actual model)`, outcome, Usage state, and time range remain usable. Selecting a model also selects its Surface.
- Request records use cursor pagination, preserve filter state, show final outcome/status, attempt count, Token observation state, and end-to-end latency, and open a complete lifecycle detail dialog without turning numeric zero into a missing value.
- All `/web-api/usage/*` requests require an authenticated Server session, return `{ data, meta }`, reject ranges over the 32-day detail window, and treat the selected range as `[fromMs, toMs)`. Trend queries reject a granularity that would exceed 2,000 points.
- Restart recovery marks unfinished requests interrupted, and completed streaming requests retain their terminal Token and latency values after reload.
- No model-cost CRUD, USD totals, cost columns, or provider cost-limit warnings are present.
- OAuth quota remains display-only unless the upstream reports an explicit, unexpired rate-limit or exhaustion state; Share Token limits and Share Market seat pricing remain available in their owning screens.

## Settings, Auth, Router, Backup

- First setup, password login, API token, email code flow, router config, client tunnel, read-only routing status, and backup/restore are reachable.
- Settings → Advanced → Backup defaults to a 12-hour interval and 3 retained backups; changing either value takes effect without restart, and selecting Disabled stops automatic creation while retention cleanup remains active.
- Server Web has no standalone Routing tab; Settings → Advanced starts with the API Routing card, which is collapsed by default and retains the same status/actions when expanded.
- Client Tunnel Owner is read-only; saving tunnel settings changes only tunnel fields and cannot bypass verified Client Owner change.
- Settings → Share → Payout Information persists one EVM address, explicit USDC/USDT selection, and one or more BSC/Base/Arbitrum One networks; warnings prohibit secrets and identify the address as public/self-declared.
- Payout clear requires confirmation; Router outage leaves the local save active and visibly reports pending/failed sync.
- Client-only settings are absent.
- Destructive actions have clear confirmation or disabled states.
- Settings → General → Current Version can start an upgrade from both localhost and a Router Client Tunnel URL; progress logs stream without 404/401 responses before process replacement, request URLs never contain access tokens, and the UI recovers the persisted task after the expected tunnel interruption.
- Current Version shows the active server PID and a live process uptime counter; Upgrade and Restart are adjacent actions, and Restart always requires a confirmation dialog. After restart, PID and runtime instance id must change and uptime must reset, including when the server was started through `nohup`.
- Closing/reopening the progress dialog or interrupting the stream preserves the task status; a service restart resumes at the persisted task and reports the running commit or a rollback failure instead of resetting to 0%.
- Publish a new mutable `latest` release and upgrade immediately: the staged binary commit must match the release target before the old process exits; a stale asset must fail before restart, and a replacement rollback must surface its final task logs after the Client Tunnel reconnects.
- Keep a Client Tunnel page and `/web-api/events` subscription open for at least two Router lease TTL periods; renewal must retain the same connection without periodic `404 unregistered-subdomain`, `503 connection-lost`, or HTTP/2 stream errors.
- Container deployments show self-update as unavailable and direct operators to deploy a new image.
- Settings → Advanced → API Management owns the log, restart, upgrade, and runtime-diagnostics API switches; Log Management no longer contains remote API controls.
- Settings → Advanced → Provider Request Defaults persists the three timeout defaults in seconds in `server.json`; saving recompiles runtime plans without materializing inherited values into Provider records.
- Settings → Advanced → Health Checks & Model Tests persists probe timeout, retry, degradation threshold in seconds, and Claude/Codex/Gemini default test models in `server.json`. Manual, scheduled, and Router-triggered probes use the same settings and fixed minimal `ping` prompt.
- A generated debug token is displayed once, expires within the selected 1-24 hour window, can be rotated/revoked, and never appears in `server.json` or API responses as plaintext.
- Through a Router Client Tunnel URL, exact `/web-api/debug/*` endpoints accept the debug Bearer token without a Web admin session; `/web-api/invoke/*`, `/web-api/admin/*`, unknown debug paths, malformed operation IDs, and query-string tokens remain protected or rejected.
- Debug log responses redact authorization, API-key, token, cookie, password, and secret assignments and do not disclose the host log path.
- Remote restart returns an operation ID before the old process exits. After reconnect, its persisted status reports old/new PID, strategy, stage, timestamp, and a health/version success or actionable failure message.
- Remote upgrade status and stream survive the expected process/tunnel interruption by reading persisted state; disabling a capability immediately rejects new requests made with an otherwise valid debug token.

## Accounts, OAuth, Quota

- Manual/import-only account templates, refresh plan, quota refresh, Codex banked reset, Copilot/Kiro device flow, and OAuth preview/finish states are visible where supported.
- In Server mode, AuthCenter and every managed-account Provider editor use `/api/accounts/capabilities` as the authority. Loading or request failure must not expose an unverified binding control; metadata-only, deprecated, missing, or `inferenceBindingSupported=false` account types remain hidden or disabled. Static API Key/AWS credentials stay in the Provider form.
- CodeBuddy OAuth and WorkBuddy OAuth appear as separate AuthCenter entries with their local product logos, including before any account or Provider exists. Each uses the existing browser login/poll/cancel, credential preview/import, list/default/remove/logout operations and the correct product type.
- AuthCenter Buddy login/import site choices are independent per product, survive collapsing/reopening within the page, and reset on leaving/reloading the page. They never save a Provider preference or rebind a Provider. Each account row shows its own validated China/International site (unknown remains unknown), alongside the existing safe account label.
- Buddy login/import pins its starting site and locks competing actions. A completed import refreshes shared account/quota views even after closing its form, while obsolete responses cannot restore old credentials, previews, site choices, or account selection. Referenced-account removal/logout failures remain visible and preserve the account and bindings.
- Saving AuthCenter quota-refresh settings invalidates both existing Buddy quota roots only when their authoritative capabilities advertise live refresh. AuthCenter coverage auditing reports the pre-existing `trae_solo` entry gap explicitly; it is not treated as implemented or silently excluded.
- Antigravity OAuth and Agy OAuth appear as separate AuthCenter entries. Add/list/default/remove and quota refresh for `agy_oauth` update only Agy state/query rows and never display or mutate `antigravity_oauth` accounts as Agy.
- DeepSeek Account shows an optional account label and a required masked access-token field, never an email/phone plus password login. Import creates a real persisted account, selects it when opened from a Provider form, and list/default/remove survive reload; neither the token nor a password field/value is echoed after import.
- Quota refresh settings appear only when the capability matrix contains at least one `quotaCapability=live_refresh` entry with `supportsLiveQuotaRefresh=true`. Saving invalidates only authoritative live-refresh roots; imported-snapshot and cached-only accounts are not presented as live polling support.
- Claude and Grok subscription expiry uses the same monthly/yearly rule control; monthly day, yearly month/day, IANA time zone, next occurrence, automatic Grok precedence, legacy-date migration, save/clear states, and narrow viewport wrapping are verified.
- Real browser login is not shown as native until capability gates are explicitly opened after real credential validation.
- Tokens and secrets are never echoed back after save/import.

## Evidence

Record manual findings in the relevant implementation note or PR/commit summary:

- Date and commit.
- Viewport checked.
- Pages checked.
- Failures found and follow-up task IDs.

## Current Status

- 2026-08-03 non-browser validation passed: Rust format/check/test, Web typecheck/unit tests/build, Web runtime contract audit, Provider coverage audit, UI Provider matrix audit, and local HTTP smoke.
- No browser automation, screenshot test, or automated click flow was run.
- Offline release readiness remains `blocked_inputs`: real Router/OAuth/Share grant/Client Market credentials and deployment acceptance were not supplied. `RUN_TESTS=0` also records the readiness script's own local-test phase as skipped; the full local suites were run separately and passed.
- Server-owned zh/zh-TW/en/ja locale coverage remains statically validated. Human reviewers still need to check translated text fit in real viewports.
- Manual wide and narrow viewport checks remain pending for a human reviewer.

### Buddy 模型与图标

- [ ] CodeBuddy / WorkBuddy 的中国区和国际区账号分别检查：新增、编辑、未保存切换账号时，固定模型目录自动加载；旧目录和晚到响应不出现在新账号下。
- [ ] 全局与按 Claude/Codex/Gemini 固定模型均能搜索名称/ID、选择真实 ID、保留 auto 和手动输入；获取目录不自动选择或保存。
- [ ] 未选账号、加载失败、手动刷新、权威空目录、过期缓存、旧模型缺失的提示正确；失败不清空旧模型。
- [ ] 新增、旧记录、编辑、复制均显示相应产品 Logo；自定义图标优先，反复显示无渐变串色，打开旧记录不因回退图标产生未保存修改。
- [ ] Router Codex 最小文字测试达到实际请求上限时提示连接成功并保留 response.incomplete；真实错误、截断和取消继续失败，Claude/Gemini 原测试正常。

以上为待执行的人工验收项目，不代表本地单测或真实环境已通过。

### Buddy 登录与导入站点

- [ ] CodeBuddy / WorkBuddy 分别切换中国站和国际站，仅改站点即可保存；刷新、重新编辑、复制和 Server 重启后保持已保存偏好。保存失败时保留选择，取消编辑后保持原值。
- [ ] 旧记录优先采用绑定账号实际站点，无可用账号资料时显示国际站；打开页面及异步加载账号均不产生未保存修改，手动选择不被晚到数据覆盖。
- [ ] 当前绑定账号实际站点与新登录／导入偏好分别显示；切换偏好不会更换已有账号或模型，无效或世代不匹配的账号显示实际站点未知。
- [ ] 登录启动、轮询和导入请求期间锁定站点，登录与导入互斥；成功、失败、取消后解锁。导入预览随站点／产品切换清除，离开后旧响应不影响新表单。

以上站点交互仍待人工页面验收，本地单测不代替实际视口或真实双站登录验证。
