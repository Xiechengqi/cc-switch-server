# Manual UI Checklist

This checklist is the UI parity gate for the cc-switch desktop Server version.
Do not replace it with Playwright, Cypress, Puppeteer, Selenium, browser screenshot
scripts, or automated click flows.

## Scope

- Compare the server Web UI against `/data/projects/cc-switch` desktop UI for the token server main path only.
- Retained pages: Providers, Shares, Usage/Pricing, Settings/Auth/Router/Backup, Accounts/OAuth/quota.
- Excluded from server (must not appear): Universal Providers, import-current-CLI-config, skills, MCP, OpenClaw workspace/tools/agents, Hermes, OMO, Tauri shell, updater, deeplink, Claude Desktop profile writing, WebDAV/S3 sync, speedtest, local CLI session parsing, `codex_responses_ws`.

## Viewports

Run the checks manually at:

- Desktop width: 1366x768 or wider.
- Narrow width: around 390px wide.

## Global

- Shell renders without blank first screen after setup/login state is known.
- Navigation labels do not wrap into unreadable text or overlap icons.
- Topbar actions remain clickable and do not cover page content.
- Tables scroll horizontally when needed instead of overflowing the viewport.
- Buttons and inputs keep stable dimensions while loading or changing state.
- No server-only hidden/excluded feature is visible.
- No browser console or network error is ignored during manual inspection.

## Providers

- Provider list, current provider state, readiness, health, model, account binding, and quota/capability summaries are visible.
- Create/edit/test/fetch-models/switch actions match server capability gates.
- Add/Edit only offers Claude, Codex, and Gemini Profiles from the Server registry; OpenCode, OpenClaw, Hermes, Claude Desktop, raw env/TOML/auth editors, automatic failover, and outbound proxy controls never appear.
- Fixed Profile identity cannot be changed by editing a name, URL, category, or compatibility metadata. Custom protocol/auth changes require preview/apply rebind; legacy records expose adopt or clone-as-custom actions instead of an unrestricted raw editor.
- Static secrets use keep/replace/clear without echoing the stored value. Managed Providers require an explicitly selected compatible account, and account/quota refresh does not mark the Provider draft dirty.
- Provider and Share edits have independent dirty/save state. Saving or failing one section does not silently commit or reset the other, and the Provider Save button is disabled when the canonical draft has not changed.
- S1 installations show a read-only migration status and blocker codes. The Web UI never applies, rolls back, or cleans the Provider store while the Server is running; it directs the operator to the offline CLI.
- Planned or diagnostic-only provider combinations are clearly gated and not presented as fully native.
- Unknown legacy JSON is preserved read-only or blocks S2 cutover; it is not exposed as an editable Server field.

## Shares

- Share status, owner, tunnel/subdomain, provider binding, ACL, limits, market/grant, pending edits, and connect info are visible.
- Share Owner is read-only and always displays Client Owner; Provider Share create/save requests do not submit an independent owner. Changing Client Owner through verified email ownership updates every Share and preserves a valid previous owner as shared access.
- Pause/resume/binding/tunnel actions are disabled or gated consistently with server state.
- Share connect info can be inspected without exposing hidden desktop-only features.
- The full Shares page scrolls vertically to the bottom at both target viewports; expanding settings or request logs does not leave content clipped below the shell.
- Request logs show the selected Share's recent seven-day history with correct token, status, latency, range, and pagination values; the table remains horizontally scrollable on narrow screens.
- After a server restart, requests written since the last usage snapshot still appear, and a completed streaming request keeps its final token and latency values.

## Usage And Pricing

- Summary, trends, logs, detail, provider stats, model stats, cache/billed tokens, and cost fields match server API names.
- Filters for app, provider, share, user, source, session, health, stream, and time window remain usable.
- Pricing model CRUD and provider limits warnings are visible only where supported.

## Settings, Auth, Router, Backup

- First setup, password login, API token, email code flow, router config, client tunnel, read-only routing status, and backup/restore are reachable.
- Client Tunnel Owner is read-only; saving tunnel settings changes only tunnel fields and cannot bypass verified Client Owner change.
- Settings → Share → Payout Information persists one EVM address, explicit USDC/USDT selection, and one or more BSC/Base/Arbitrum One networks; warnings prohibit secrets and identify the address as public/self-declared.
- Payout clear requires confirmation; Router outage leaves the local save active and visibly reports pending/failed sync.
- Desktop-only settings are absent.
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

- 2026-07-03 static-only pass: not run in a browser.
- Reason: current implementation pass prohibits deployment/startup and UI automation.
- Static gate used: `scripts/static-checks.sh`; native invoke registry audit currently reports no registered-not-implemented command and checks implemented commands against `web_invoke_dispatch`.
- Phase M/M1+N2 i18n static pass is implemented: language switch is in Settings, the desktop zh/zh-TW/en/ja locale files are copied as the migration baseline, page-level and Dashboard body copy use the lightweight runtime dictionary/`tx()` layer, and `scripts/audit/audit-web-i18n-literals.mjs` currently reports zero JSX English literals. Human reviewers still need to check translated text fit in real viewports.
- Manual desktop and narrow viewport checks remain pending for a human reviewer.
