# Manual UI Checklist

This checklist is the UI parity gate for the cc-switch desktop Server version.
Do not replace it with Playwright, Cypress, Puppeteer, Selenium, browser screenshot
scripts, or automated click flows.

## Scope

- Compare the server Web UI against `/data/projects/cc-switch` desktop UI for the token server main path only.
- Retained pages: Providers, Shares, Usage/Pricing, Settings/Auth/Router/Backup, Universal Providers, Accounts/OAuth/quota.
- Hidden or excluded areas must not appear in navigation or primary actions: skills, MCP, OpenClaw workspace/tools/agents, Hermes, OMO, Tauri shell, updater, deeplink, Claude Desktop profile writing, WebDAV/S3 sync, speedtest, local CLI session parsing, `codex_responses_ws`, Codex OAuth image generation.

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
- Planned or diagnostic-only provider combinations are clearly gated and not presented as fully native.
- Advanced JSON data is not lost when editing common fields.

## Shares

- Share status, owner, tunnel/subdomain, provider binding, ACL, limits, market/grant, pending edits, and connect info are visible.
- Pause/resume/binding/tunnel actions are disabled or gated consistently with server state.
- Share connect info can be inspected without exposing hidden desktop-only features.

## Usage And Pricing

- Summary, trends, logs, detail, provider stats, model stats, cache/billed tokens, and cost fields match server API names.
- Filters for app, provider, share, user, source, session, health, stream, and time window remain usable.
- Pricing model CRUD and provider limits warnings are visible only where supported.

## Settings, Auth, Router, Backup

- First setup, password login, API token, email code flow, router config, client tunnel, upstream proxy, and backup/restore are reachable.
- Desktop-only settings are absent.
- Destructive actions have clear confirmation or disabled states.

## Universal Providers

- Universal provider list, model catalog, model mapping, app enablement, sync, import, and export are reachable.
- Derived provider cleanup behavior is represented in the UI wording or action result.

## Accounts, OAuth, Quota

- Manual/import-only account templates, refresh plan, quota refresh, Codex banked reset, Copilot/Kiro device flow, and OAuth preview/finish states are visible where supported.
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
