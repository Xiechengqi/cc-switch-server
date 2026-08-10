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

- The page lists Provider Bundles rather than separate per-App Provider records. Each Bundle title shows the logos for every supported Claude, Codex, and Gemini Surface.
- Add Provider starts with one Family selector sourced from the Server Provider registry. Selecting a Family automatically creates its complete authoritative Surface set; the operator never adds or removes arbitrary App records.
- Every visible, creatable Family appears exactly once, including Custom HTTP, and the matrix matches the provider coverage audit. OpenCode, OpenClaw, Hermes, Claude Desktop, Universal Providers, raw env/TOML editors, automatic failover, and outbound proxy controls never appear.
- Selecting Grok OAuth automatically shows Claude, Codex, and Gemini as icon-labelled tabs. Switching tabs does not discard unsaved values or change Bundle-wide fields.
- Bundle name, Family identity, OAuth/managed account, shared credentials, common endpoint, shared driver options, and Remote Share controls remain outside the Surface tabs.
- Model policy control defaults to Global below credentials and outside the Surface tabs. Global applies one policy to every configurable Surface; fixed Profiles remain read-only exceptions and are identified in the global summary. Per-App control is offered only when at least two Surfaces are configurable.
- Each Surface tab contains an independent model policy only in Per-App mode. In Global mode it contains only that App's enable state, custom endpoint override when the Family permits it, protocol/auth configuration when custom binding permits it, Surface credential slots and headers, test model, and typed request/stream timeouts.
- No editable raw Provider JSON, env object, TOML, `settingsConfig`, `meta`, cache/thinking/governance switch, image model, or video model appears. Saved Surfaces may expose the secret-free compiled Runtime Plan under a collapsed, read-only Effective Runtime Configuration diagnostic with a copy action.
- Custom Header and Query authentication require an explicit header or query-parameter name. Custom Header names, extra Header names, URL fields, timeout ranges, and test-model length reject invalid values before save and are revalidated by the Server.
- Managed Families require one explicitly selected compatible Account shared by their enabled Surfaces. Account and quota refresh updates do not mark unrelated Bundle fields dirty.
- Static secrets use authenticated slot-scoped reveal and password controls. Opening or revealing a secret does not dirty the draft; edits submit `replace`, an unchanged secret submits `keep`, and clearing an optional secret submits `clear`. List and detail responses remain redacted.
- Disabling a Surface removes it from the Bundle's enabled App set. That Surface requires no credential, is absent from the active runtime index and routes, and is excluded from Bundle Share bindings; a read-only compile diagnostic may still be returned for editing.
- Fixed Family/Profile identity, canonical website, endpoint, protocol, and auth fields cannot be changed through display-name or metadata edits. Custom HTTP exposes only the explicit custom controls allowed by the registry.
- The editor has exactly one global Save action at the bottom. There are no per-tab Save buttons, and saving submits the complete Bundle plus the one Bundle-scoped Share configuration.
- Create/edit/test/fetch-model actions follow the capability gates of the active Surface without introducing a current/selected Provider state.
- Bundle cards and the editor expose no Switch, Select, Set Current, Clear Current, or hot-switch action. Runtime routing is resolved from the request route, Bundle Surface, or Share binding.
- Bundle cards show the explicit Global or Per-App scope and each enabled App's effective model policy. A Global summary is compacted once, while fixed Profile exceptions remain labelled separately and long model names truncate without resizing actions.
- Saving, reloading, editing, and deleting a Bundle preserve its complete Surface set and revision. A Bundle referenced by a Share cannot be deleted until the reference is removed.

## Shares

- Share status, owner, tunnel/subdomain, provider binding, ACL, limits, market/grant, pending edits, and connect info are visible.
- One Provider Bundle maps to at most one Share. All enabled Claude, Codex, and Gemini Surface bindings use that same Share record, subdomain, and Share URL; no per-App Share URL is created.
- The Bundle editor keeps Remote Share outside the Surface tabs and uses the same bottom Save action. The Server derives bindings from enabled Surfaces instead of trusting App/provider binding fields from the browser.
- Enabling or disabling a Bundle Surface and saving reconciles that one Share's bindings without changing its URL, ACL, limits, sale settings, or tunnel identity.
- Share Owner is read-only and always displays Client Owner; Provider Share create/save requests do not submit an independent owner. Changing Client Owner through verified email ownership updates every Share and preserves a valid previous owner as shared access.
- Pause/resume/binding/tunnel actions are disabled or gated consistently with server state.
- Share connect info can be inspected without exposing excluded client-only features.
- The full Shares page scrolls vertically to the bottom at both target viewports; expanding settings or request logs does not leave content clipped below the shell.
- Request logs show the selected Share's recent seven-day history with correct token, status, latency, range, and pagination values; the table remains horizontally scrollable on narrow screens.
- After a server restart, requests written since the last usage snapshot still appear, and a completed streaming request keeps its final token and latency values.
- User Token periods show Lifetime, Daily, UTC calendar week, Every 7 days, Calendar month, and Every 30 days. The two fixed periods require a non-future UTC start time, preserve minute precision, and hide/clear that field for all calendar periods.
- Two Share users with the same fixed period but different starts show independent current windows and reset countdowns. Changing a user's period or start recomputes the current window from request history instead of resetting usage to zero.

## Usage

- Overview cards and trends use the same half-open time range and global filters as every table. Request count includes only user inference; supplemental calls add Token volume without adding requests; health probes stay absent.
- The four tabs are Requests, Providers, Models, and Share / Users. Provider rows aggregate a Bundle once and show its Claude/Codex/Gemini Surface breakdown; models are grouped by `(Surface, actual upstream model)`.
- Filters for Claude/Codex/Gemini Surface, Provider Bundle, Share, normalized user email, `(Surface, actual model)`, outcome, Usage state, and time range remain usable. Selecting a model also selects its Surface.
- Request records use cursor pagination, preserve filter state, show final outcome/status, attempt count, Token observation state, and end-to-end latency, and open a complete lifecycle detail dialog without turning numeric zero into a missing value.
- All `/web-api/usage/*` requests require an authenticated Server session, return `{ data, meta }`, reject ranges over the 32-day detail window, and treat the selected range as `[fromMs, toMs)`. Trend queries reject a granularity that would exceed 2,000 points.
- Restart recovery marks unfinished requests interrupted, and completed streaming requests retain their terminal Token and latency values after reload.
- No model-cost CRUD, USD totals, cost columns, or provider cost-limit warnings are present.
- OAuth quota remains display-only unless the upstream reports an explicit, unexpired rate-limit or exhaustion state; Share Token limits and Token Market sale pricing remain available in their owning screens.

## Settings, Auth, Router, Backup

- First setup, password login, API token, email code flow, router config, client tunnel, read-only routing status, and backup/restore are reachable.
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
- A generated debug token is displayed once, expires within the selected 1-24 hour window, can be rotated/revoked, and never appears in `server.json` or API responses as plaintext.
- Through a Router Client Tunnel URL, exact `/web-api/debug/*` endpoints accept the debug Bearer token without a Web admin session; `/web-api/invoke/*`, `/web-api/admin/*`, unknown debug paths, malformed operation IDs, and query-string tokens remain protected or rejected.
- Debug log responses redact authorization, API-key, token, cookie, password, and secret assignments and do not disclose the host log path.
- Remote restart returns an operation ID before the old process exits. After reconnect, its persisted status reports old/new PID, strategy, stage, timestamp, and a health/version success or actionable failure message.
- Remote upgrade status and stream survive the expected process/tunnel interruption by reading persisted state; disabling a capability immediately rejects new requests made with an otherwise valid debug token.

## Accounts, OAuth, Quota

- Manual/import-only account templates, refresh plan, quota refresh, Codex banked reset, Copilot/Kiro device flow, and OAuth preview/finish states are visible where supported.
- In Server mode, AuthCenter and every managed-account Provider editor use `/api/accounts/capabilities` as the authority. Loading or request failure must not expose an unverified binding control; metadata-only, deprecated, missing, or `inferenceBindingSupported=false` account types remain hidden or disabled. Static API Key/AWS credentials stay in the Provider form.
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
- Offline release readiness remains `blocked_inputs`: real Router/Market/OAuth/Share credentials and deployment acceptance were not supplied. `RUN_TESTS=0` also records the readiness script's own local-test phase as skipped; the full local suites were run separately and passed.
- Server-owned zh/zh-TW/en/ja locale coverage remains statically validated. Human reviewers still need to check translated text fit in real viewports.
- Manual wide and narrow viewport checks remain pending for a human reviewer.
