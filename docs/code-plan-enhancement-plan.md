# Code Plan Enhancement Plan

This plan covers every `cc-switch-server` Code Plan score below 9 in
`/data/projects/proxy/proxy.md`. External repositories are protocol evidence,
not synchronization sources. Every implementation keeps one explicit Account
bound to one Provider/Share surface. Account pools, rotation, cross-account
retry, balance selection, shadow accounts, and payment features are out of
scope.

## Global Invariants

- Resolve the Provider and exact Account before execution. A retry may refresh
  and replay that Account once, but may not select another Account or Provider.
- Fence credential, project, entitlement, session, and task state by Provider
  type, Account id, and `authIdentityGeneration`; fence token refresh commits by
  `tokenRefreshGeneration` and the credential snapshot.
- Persist account changes only through `ServerStateInner` domain methods in
  `state.rs`. Domain-only transformations remain in `domain`.
- Treat checked-in fixtures and local tests as protocol evidence. Keep live
  acceptance pending until a real credential exercises the documented route.
- Record every adopted external behavior in `UPSTREAM_IMPORT.md` and keep
  `docs/provider-coverage.md` aligned with the runtime contract.

## Scoring Queue and References

| Order | Code Plan | Baseline | Reference 1 | Reference 2 |
| ---: | --- | ---: | --- | --- |
| 1 | Gemini / Google Code Assist | 8.6 | TokenRouter 9.2 (`3141e7622`) | CLIProxyAPI 9.0 (`bd34ceca`) |
| 2 | Grok / xAI | 8.8 | TokenRouter 9.5 (`3141e7622`) | sub2api 9.3 (`1e618dbc`) |
| 3 | Antigravity / Agy | 8.4 | OmniRoute 9.2 (`e6523da2`) | TokenRouter 9.2 (`3141e7622`) |
| 4 | GitHub Copilot | 6.5 | OmniRoute 8.8 (`e6523da2`) | cc-switch 8.4 (`b1dee015`) |
| 5 | Kiro / Amazon Q | 8.0 | OmniRoute 9.0 (`e6523da2`) | 9router 8.2 (current audited snapshot) |
| 6 | Cursor | 6.8 | OmniRoute 9.0 (`e6523da2`) | 9router 7.8 (current audited snapshot) |
| 7 | Kimi Code | 8.0 | OmniRoute 9.0 (`e6523da2`) | CLIProxyAPI 8.5 (`bd34ceca`) |
| 8 | Qoder / COSY | 0.0 | TokenRouter 9.3 (`3141e7622`) | 9router 8.8 (current audited snapshot) |
| 9 | API Key Coding Plans | 8.0 | OmniRoute 9.0 (`e6523da2`) | TokenRouter 8.8 (`3141e7622`) |

Reference commits are rechecked at the start of each implementation phase. A
score tie is resolved in favor of the project with the narrower and more
testable protocol evidence, while both tied leaders remain documented when
relevant.

## 1. Gemini / Google Code Assist

### Verified Current State

- `GeminiCli`, `AntigravityOAuth`, and `AgyOAuth` already share the Code Assist
  `v1internal` generation envelope while retaining separate identity metadata.
- Project discovery is account-scoped single-flight, retries one same-account
  401 after native refresh, and commits only when the credential snapshot and
  identity/token generations still match.
- Thought signatures already survive native Gemini, Claude Messages, OpenAI
  Responses, and Chat paths, including streaming tool signatures.
- Real Google OAuth/Code Assist acceptance is still external and must remain
  `live_pending`.

### Reference Evidence

- TokenRouter separates Code Assist/Google One/AI Studio OAuth and Vertex
  service-account rails, persists project/tier discovery, checks token versions
  before caching, and treats model quota as entitlement/capacity evidence.
- CLIProxyAPI separates API-key Gemini, AI Studio, Vertex, and Antigravity
  executors; it also contains strict signature replay/validation and translator
  fixtures. Its scheduler and account fallback are intentionally not adopted.

### Verified Implementation

- Add a reusable non-secret capability-evidence model with dimensions for
  credential rail, project provisioning, and model entitlement.
- Store only normalized state/source/reason/time and the originating
  `authIdentityGeneration`; never store raw upstream payloads in this evidence.
- Record project evidence from `loadCodeAssist` and model entitlement evidence
  from `retrieveUserQuota` through existing generation-fenced state commits.
- Project the evidence on `AccountPublicView` as
  `supported/unsupported/unknown/stale`, including `fresh/stale/superseded`
  freshness. Token presence alone must never imply entitlement.
- Keep AI Studio API Key explicitly separate from Code Plan entitlement. Do not
  claim Google One, AI Studio OAuth, or Vertex subscription support until a
  dedicated credential contract and driver are implemented and accepted.

### Acceptance

- Domain tests cover missing evidence, current-generation evidence, expiry,
  superseded generation, and API-key unsupported semantics.
- Quota/project tests prove the authoritative calls emit observation drafts.
- API serialization proves secrets and project identifiers are absent.
- Run `cargo fmt --check`, targeted library tests, `cargo test`, Web typecheck,
  provider audits, smoke tests, and offline release readiness.

### Verified Acceptance

- The current library inventory contains 142 `gemini`-filtered tests, and the
  full Rust suite passes them with the current-generation project,
  entitlement, redaction, signature, quota, and same-account refresh fixtures.
- Static Provider/UI audits and API serialization tests pass. Real Google
  OAuth, project provisioning, quota, and inference remain `live_pending`
  because no real credential was supplied.

## 2. Grok / xAI

### Verified Reference Evidence

- TokenRouter `3141e76222b7f4822c22ffa9e082b933bcb34310` was audited for
  video-task ownership, media entitlement fail-closed behavior, endpoint
  capability probes, SSE ping normalization, search accounting, and OAuth
  reconciliation. Its reconciler scans and mutates account pools, so only the
  per-bound-account protocol evidence was adopted.
- sub2api moved from the planned `b74024c7868ee88a0bf921306cbc22a2f922872a`
  snapshot to `1e618dbc299fc0a82e9a690bcf2d5843be817113` before this phase.
  The current commit was re-audited for Grok model-not-found classification,
  entitlement drift, search usage, and reconciliation. No scheduler, account
  selector, soft gate, billing, or payment code was imported.

### Verified Implementation

- Added current-generation capability evidence for WebSocket, image, image
  edit, video, hosted search, and media entitlement. Evidence has explicit
  positive/negative/unknown state, TTL, and `authIdentityGeneration`; legacy
  `profile.grokCapabilities` is a stale diagnostic projection and cannot grant
  access.
- Billing/quota observations now authorize media only from complete current
  paid evidence. Explicit 403 entitlement rejection and unsupported endpoint
  responses write negative evidence; 401, 429, and 5xx remain inconclusive.
- Responses SSE ping records are normalized, bounded by 64 KiB and 128 lines,
  and hosted search completion is deduplicated by stable item id across
  `item.done` and `response.completed`. Non-stream Responses also records search
  evidence and usage.
- Added the durable `grok-media-tasks.json` ownership store. A video task is
  bound to Provider, exact Account, `authIdentityGeneration`, Share, runtime,
  user namespace, task id, and a seven-day TTL. The store is bounded to 4096
  entries, restored on startup, included in backup/restore validation, and
  fails closed on corrupt JSON, schema drift, excessive TTL, or persistence
  failure. Cross-Share/user lookup is 404; identity/runtime drift is 409.
- Provider test now exposes a Grok-only reconciliation report. A dry run is
  local and read-only; network mode may refresh only the already bound Account
  using the expected identity generation, then probes only that Provider's
  Responses endpoint/model. Refresh rejection returns a redacted structured
  result and stops before the model probe. Endpoint/model status is conservative:
  only explicit structured model rejection is `unsupported`; auth, rate limit,
  ambiguous 4xx, and 5xx remain `inconclusive`.

### Verified Acceptance

- `cargo fmt --all -- --check` and `cargo check` pass.
- `cargo test grok_ --lib` passes 121 tests, including current-generation
  evidence, quota/media fail-closed behavior, persistent task ownership,
  Responses/SSE/WS/search/media paths, same-account refresh/replay, and
  reconciliation failure paths.
- `cargo test provider_reconciliation --lib` passes all three integration
  tests. They prove dry-run makes zero token/Responses requests, network mode
  preserves identity while advancing only `tokenRefreshGeneration`, stale
  identity blocks before outbound traffic, and refresh rejection never reaches
  Responses or exposes upstream credentials/errors.
- Web typecheck, provider coverage audit, UI provider matrix audit, and
  `git diff --check` pass. Real xAI OAuth, quota, Responses, WS, search, and
  media acceptance remains `live_pending` because no real credential was used.

## 3. Antigravity / Agy

### Verified Reference Evidence

- OmniRoute moved from the planned `e6523da2` snapshot to
  `918fba5e392ce8b137976349f035597196edc440`. The current commit was audited
  for project bootstrap/persist, final header scrub, Claude/Gemini quota-family
  grouping, and structured 429 classification. Its scheduler, account fallback,
  browser pool, and long automatic retry loops are excluded.
- TokenRouter `3141e76222b7f4822c22ffa9e082b933bcb34310` was audited for
  read/write privacy verification, subscription tier and model-quota details,
  Google RPC `ErrorInfo`/`RetryInfo`, schema cleanup, image contracts, thinking
  signatures, and single-account tests. Its account switching, scheduler,
  sticky-account clearing, payment, and sixty-attempt capacity loop are
  excluded. Server will observe privacy read-only; it will not silently mutate
  an external account setting.

### Verified Implementation

- Retain the existing generic `gemini_code_plan` projection and add a distinct
  `antigravity_code_plan` projection for both Antigravity and Agy rails. The
  projection has current-generation `project_bootstrap`, `privacy`,
  `tier_entitlement`, `gemini_quota_family`, `claude_quota_family`, and
  `model_capacity` dimensions. Agy and Antigravity never share Account state.
- Emit project/tier/family observations from the existing `loadCodeAssist` and
  `retrieveUserQuota` calls. Parse family support from explicit model ids only;
  an absent family remains unknown rather than unsupported. Use a bounded,
  read-only `fetchUserInfo` observation when a project is available. Never
  persist raw project ids or upstream payloads in capability evidence.
- Apply an Antigravity/Agy-only final header scrub after Account overrides and
  before transport. Remove inbound proxy/browser/Stainless/request-signature
  fingerprints while retaining only the Server-generated Antigravity identity,
  protocol authentication, and required content negotiation headers.
- Strictly parse Google RPC errors only when status/reason pairs match
  `RESOURCE_EXHAUSTED` + `RATE_LIMIT_EXCEEDED` or `UNAVAILABLE` +
  `MODEL_CAPACITY_EXHAUSTED`. Accept only bounded non-negative protobuf duration
  strings from `RetryInfo.retryDelay`; normalize model scope from structured
  metadata, never free text.
- A short delay may replay once using the same `ProviderExecution`, Account id,
  `authIdentityGeneration`, Share/runtime namespace, and prepared request before
  any downstream bytes are committed. A long delay writes only the current
  Share/runtime family/model cooldown and returns the upstream error. Missing or
  malformed structured evidence is not retryable. No path selects another
  Account or Provider.

- The successful quota path now carries the project/tier observations produced
  by `loadCodeAssist`; parse failures retain the same snapshot as a partial
  update. Capacity is inferred only from buckets with an explicit non-empty
  model id, so anonymous quota buckets cannot create false capacity evidence.
- The existing Share model cooldown gate now covers Antigravity and Agy as
  well as Codex. Cooldown keys use the normalized model from structured
  `ErrorInfo`, never a caller-provided alias. Capability evidence persistence
  may rebuild the Provider runtime, but retry revalidation requires the same
  Provider revision/runtime fingerprint and Account identity generation before
  a second outbound request.

### Acceptance

- Domain/API tests prove two independent projections, current-generation
  fencing, expiry, absent-family unknown state, and redaction.
- Quota tests prove project, tier, Gemini/Claude family, privacy, and model
  observations are emitted without mutating external settings.
- Header contract tests prove spoofable browser/proxy/Stainless headers cannot
  survive the final Antigravity/Agy identity step.
- Fixture tests cover valid/invalid `ErrorInfo` and `RetryInfo`, bounded delay,
  same-account replay-once, long-delay cooldown, stale generation, and both
  Claude and Gemini model families.
- Run formatting, compile, targeted Antigravity tests, Web typecheck, provider
  audits, local smoke, and offline readiness. Real OAuth/privacy/quota/inference
  acceptance remains `live_pending` without supplied credentials.

### Verified Acceptance

- `cargo test antigravity --lib` passes 24 tests. The suite covers both
  Antigravity and Agy projections, generation supersession and expiry,
  project/tier/family/capacity observations, empty and telemetry-bearing
  privacy settings, privacy probe failure, final header replacement, strict
  Google RPC parsing, Gemini 429 and Claude 503 same-account replay, repeated
  limit termination, long-delay Share/runtime/model cooldown, and identity
  drift before replay. The fixtures assert that no `setUserSettings` request is
  made and that the second outbound request count is zero after generation
  drift or an active model cooldown.
- `cargo fmt --check`, `cargo check`, Web typecheck, provider coverage audit,
  UI provider matrix audit, `git diff --check`, and the local HTTP smoke pass.
  The first smoke invocation timed out while linking the executable target;
  after that target completed normally, the actual health/setup/auth/Provider/
  Share smoke passed on a fresh port and config directory.
- `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` reports
  `verificationState=blocked_inputs` with zero failures, as required: local
  tests were deliberately omitted in that negative audit and real Server,
  Share, Router, Market, Claude, Codex, Gemini, and deployment inputs were not
  supplied. No live acceptance is claimed.
- `UPSTREAM_IMPORT.md`, generated provider coverage documentation/JSON, and
  its audit source record the audited references, included behavior, explicit
  exclusions, local fixture status, and live gate.

## 4. GitHub Copilot

### Verified Reference Evidence

- The references were re-audited at their current local heads instead of the
  hashes originally recorded in the queue: OmniRoute
  `918fba5e392ce8b137976349f035597196edc440` and cc-switch
  `c0050623194303ecc95c3ce7ca8e362bce21e762`.
- OmniRoute's `open-sse/services/githubCopilotModels.ts` discovers the
  per-account public catalog from the Copilot API host, distinguishes the GHE
  `endpoints.api` chat catalog from `endpoints.proxy`, and intentionally avoids
  a static fallback for enterprise-specific model IDs. Its
  `open-sse/services/usage/github.ts` distinguishes paid `quota_snapshots`
  from free/limited `monthly_quotas` + `limited_user_quotas`, preserves
  `unlimited`, and carries the authoritative reset window.
- OmniRoute's GitHub and GHE OAuth providers both exchange the GitHub OAuth
  token through `copilot_internal/v2/token`; the GHE response supplies the
  instance-specific API/proxy endpoints. Its proactive health check treats the
  Copilot sub-token as a separate short-lived credential.
- cc-switch's `copilot_auth.rs` provides the same github.com/GHES device-flow
  split, per-account refresh single-flight, dynamic endpoint discovery,
  per-account model cache, and premium-interaction quota shape. Its desktop
  default-account selector is explicitly excluded: Server execution remains
  bound to one Account ID and identity generation.

### Concrete Gap and Implementation

- Replaced the existing GHE shortcut that forwarded the long-lived GitHub OAuth
  token to the Copilot data plane. Both github.com and GHES must mint and cache
  a short-lived Copilot token from the domain-specific
  `copilot_internal/v2/token` endpoint. A 401 may exchange and replay once only
  for the same Account and the same `authIdentityGeneration`.
- Parse endpoint metadata from both token and internal-user responses, then
  validate it before use. Production endpoints must be HTTPS, contain no
  username/password/query/fragment, use the origin root, and match either the
  public Copilot hosts or the exact GHES-derived Copilot host family. An
  untrusted discovered endpoint fails closed; it is never converted into an
  arbitrary Server-side request target.
- Added a generation-scoped Copilot model catalog. Fetch `<validated-api>/models`
  with the Copilot bearer token and official editor/plugin identity, retain
  only non-empty unique model IDs, and publish source/fetch time. Public
  github.com may use the audited static catalog only as explicitly stale
  last-known compatibility evidence; GHES never receives a public static
  fallback because its model IDs are instance-specific. Cache keys include
  Account ID, auth identity generation, token refresh generation, domain, and
  validated API origin.
- Replaced imported-snapshot-only quota refresh with a live
  `copilot_internal/user` request using the GitHub OAuth token. Support both
  paid `quota_snapshots.premium_interactions` and free/limited monthly quota
  shapes, preserve `unlimited`, clamp remaining counts and percentages, and
  retain the upstream reset time. Token presence alone remains insufficient to
  claim subscription or model entitlement.
- Published a `github_copilot_code_plan` capability projection with separate
  credential-flow, token-exchange, endpoint-provenance, model-catalog, and
  premium-interactions dimensions. All observed evidence is fenced by
  `authIdentityGeneration` and expires; a failed or absent probe remains
  unknown/unsupported rather than becoming ready by inference.
- Enabled Driver discovery after the bound-account implementation and added a
  first-class `codex.github_copilot` Profile. Claude and Codex use per-app
  single-model defaults (`claude-sonnet-5` and `gpt-5.5`) while sharing the
  family credential binding. Added fixtures for github.com and GHES token
  exchange, hostile endpoint
  rejection, generation supersession, cache invalidation, model parsing,
  paid/unlimited/free quota shapes, Anthropic-to-Chat and Responses-to-Chat
  conversion, streaming/tool behavior, and same-account 401 replay. M365
  Copilot, Copilot Web, account selection, pools, and cross-account retry are
  explicitly outside this phase.

### Verified Acceptance

- No GitHub OAuth token reaches a Copilot inference or model endpoint; only the
  short-lived exchanged token does.
- Endpoint discovery cannot turn account-controlled response data into an
  arbitrary HTTPS target, and GHES endpoints remain tied to the configured
  enterprise domain.
- Model and quota evidence cannot survive Account identity replacement and
  cannot be borrowed from another account.
- Public and GHES fixture suites pass for Chat/Responses/tools/streaming,
  discovery, quota, and same-account refresh. Live github.com and GHES commands
  are documented but remain unverified until real credentials are supplied.
- `cargo test copilot --lib` passes all 48 Copilot tests. The forwarder fixtures
  cover Claude non-stream/stream tool lifecycles, Codex Responses non-stream/
  stream tool lifecycles, and a single same-account 401 exchange/replay. They
  assert that inference receives only the short-lived Copilot token.
- Registry generation now exposes Claude and Codex Copilot surfaces, Driver
  discovery is fixture-verified, and the regression matrix classifies those
  two surfaces as fixture-verified Native. Gemini remains fallback. Real
  github.com/GHES device flow, quota, model discovery, and inference remain
  `live_pending`.
- `AdapterCapability.supports_oauth_refresh` remains a backward-compatible
  adapter-transform flag and is uniformly false; `/api/accounts/capabilities`
  and the same-account execution layer remain the authoritative refresh truth.
- `scripts/static-checks.sh` passes end to end: rustfmt and Clippy are clean,
  all 34 Node audit tests and every provider/UI/product-boundary audit pass,
  Web typecheck passes, and all 32 Web test files (164 tests) pass.
- The final current-worktree `cargo test` passes 2,314 library tests, 121 API
  contract tests, the lease payload integration test, and doc tests. The
  `src/proxy/forwarder.rs` and `src/state.rs` hashes are identical before and
  after the suite, proving the result was not taken across a concurrent source
  change.
- `scripts/smoke/smoke-local.sh` passes health, version, embedded Web, setup,
  password/API-token authentication, Provider creation, and Share creation on
  a fresh temporary data directory.
- `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` reports zero failures
  and the expected `blocked_inputs` state: local tests were intentionally
  omitted from that negative readiness invocation, and Router/Market/OAuth and
  deployment inputs were not supplied. No github.com or GHES live acceptance
  is claimed.

## 5. Kiro / Amazon Q Developer

### Reference Audit

- Highest-scoring references: OmniRoute 9.0 at
  `918fba5e392ce8b137976349f035597196edc440` and 9router 8.2 at
  `15223724c3e1ad898e84ef6e0cc1686cbafc8290`.
- OmniRoute's `open-sse/services/kiroRegion.ts`, Kiro usage/model services,
  OAuth providers, refresh path, executor, and cross-region tests separate the
  IAM Identity Center/OIDC token region from the Amazon Q profile/runtime
  region. `profileArn` is authoritative for runtime calls. Profile discovery
  probes the documented `us-east-1` and `eu-central-1` profile regions first,
  then one strictly validated IdC region as a bounded forward-compatible
  candidate.
- 9router's Kiro executor, OAuth/import surfaces, model and usage services, and
  profile tests provide useful login-source, quota, and diagnostic fixtures.
  Its current executor still permits stored region/endpoint fallback to drive
  runtime routing, so that behavior is evidence to test against, not a design
  to copy.
- The Server already has Builder ID, IdC, Google/GitHub Social, External IdP,
  API-key import, refresh, model discovery, quota, strict AWS EventStream, and
  Claude/Codex bridges. The concrete defect is inconsistent region authority:
  login, import, refresh, model, quota, and inference can independently prefer
  legacy `apiRegion`, causing a valid cross-region IdC account to call the wrong
  runtime endpoint.

### Implementation Plan

1. Add a shared Kiro region/provenance contract in
   `src/domain/providers/kiro.rs`: strict AWS region validation, strict
   CodeWhisperer profile-ARN parsing, an authoritative runtime-region resolver,
   and a bounded profile-discovery candidate order. Keep `apiRegion` as a
   compatibility alias only; it must never override a valid profile ARN.
2. Apply the contract to Builder ID/IdC/social login, API-key validation,
   credentials import, and refresh. OIDC registration/token calls use only
   `authRegion`; profile, initial usage, model, quota, and inference calls use
   only the resolved runtime region. Persist `runtimeRegion` and
   `profileProvenance`, while mirroring the resolved value into `apiRegion` for
   backward readers.
3. Discovery is sequential and bounded to at most three trusted AWS regions:
   the two known profile regions ordered by geography, then the validated auth
   region if distinct. Accept only a syntactically valid CodeWhisperer profile
   ARN and derive runtime region from that ARN. An explicitly supplied invalid
   ARN fails closed; no arbitrary endpoint or hostname enters production.
4. Make model discovery, quota refresh, token-refresh usage backfill, and
   inference consume the same resolver. A stored `apiRegion` mismatch is
   repaired/ignored when a valid ARN exists; an invalid ARN cannot silently
   fall back to a different identity or region.
5. Expand quota parsing from the first breakdown to every resource, preserving
   resource names, free-trial/bonus contributions, reset evidence, and an
   explicit connected-but-unavailable state for an empty breakdown. Do not
   synthesize an unlimited quota or use quota to select an account.
6. Add fixtures for cross-region IdC login, explicit/discovered/imported ARN
   provenance, ARN-vs-legacy-region mismatch, strict SSRF rejection, refresh
   consistency, and model/quota/inference endpoint agreement. Audit first-frame
   and terminal EventStream timeouts before changing transport behavior.
7. Update the Kiro wire contract, Provider coverage, upstream evidence log, and
   regression matrix. Run focused tests, Phase-0 contract generation/checks,
   full Rust/Web/static/smoke gates, and offline readiness. Real Kiro account
   acceptance remains `live_pending` without credentials.

### Acceptance Invariants

- One Share remains bound to one Kiro Account and one identity generation. No
  pool, rotation, quota-based selection, cross-account retry, endpoint fallback,
  or shadow identity is introduced.
- `authRegion` can affect only authentication endpoints. Once `profileArn` is
  known, its region is the sole runtime authority for profile, model, quota,
  and inference requests across the current credential generation.
- Every region interpolated into a hostname passes the shared strict validator;
  every authoritative profile ARN is structurally valid and belongs to the
  `aws:codewhisperer` namespace.
- A 401 may refresh and replay the same bound account once. Refresh cannot
  change account identity from stale legacy region metadata or overwrite a
  newer generation.

### Implemented Design

- Added one strict region/profile identity contract shared by login, import,
  refresh, model discovery, quota, and inference. `authRegion` is confined to
  OIDC. A valid CodeWhisperer profile ARN supplies the runtime region and wins
  over `runtimeRegion`/legacy `apiRegion`; every region and ARN is validated
  before it can enter an AWS hostname.
- Bounded profile discovery to the two audited US/EU profile regions plus one
  validated auth region. Builder ID and Social retain their audited shared
  profiles, API Key is explicitly profileless, and enterprise IdC/External IdP
  must resolve a real organization ARN. The historical shared enterprise ARN
  is rejected for runtime use.
- Implemented a two-receipt legacy refresh migration. The rotated token is
  durably recorded first, then the fake ARN is cleared and the account enters
  `profile_resolution_required`; successful discovery records the real ARN in
  a second receipt. Model, quota, and inference fail closed between receipts,
  while a previously real organization ARN remains protected against drift.
- Expanded usage parsing across all resource breakdowns. Only exhausted
  `AGENTIC_REQUEST` with overage disabled blocks inference; code-review quota
  never does. Trial/bonus credits, reset evidence, resource names, and an
  explicit empty-breakdown state are preserved without inventing capacity.
- Tightened AWS EventStream completion and deadlines. The first complete valid
  frame has an absolute deadline from request send; partial bytes and local
  protocol prelude do not extend it. Idle timeout resets only on complete
  frames, every successful surface requires `endEvent`, and frames after
  terminal, missing terminal, CRC corruption, or truncation fail closed. A
  timeout is a stable 504 and cannot trigger refresh/replay.
- Made model capability fail closed before cache access. Cache scope now
  includes Account ID, auth identity generation, token refresh generation,
  authoritative profile ARN (or a profileless API-key marker), and runtime
  region. Unresolved identity, absent credential, non-retryable 4xx, and a
  successful empty catalog never expose static models. Only transient failures
  after identity validation may use bounded stale/static compatibility data in
  the exact same scope.

### Verified Acceptance

- Cross-region profile/model/quota/inference fixtures use the profile ARN
  region while authentication remains on its independent IdC region. Invalid
  region/ARN input and unresolved/legacy enterprise identities stop before
  runtime requests.
- Claude Messages, Codex Chat Completions, and Codex Responses fixtures prove
  absolute first-frame timeout, terminal `endEvent`, corrupt CRC, truncation,
  and missing terminal behavior. Timeout does not refresh/replay and releases
  the account lease.
- Model fixtures prove Account/auth/token/profile/region cache isolation,
  authoritative empty results, 401 fail-closed behavior, transient 503
  fallback, and removal of configured/static models while IdC profile
  resolution is pending.
- `cargo test kiro --lib --no-fail-fast` passes 127 tests. Full static, smoke,
  offline readiness, and real-account gates are recorded separately below;
  no live Kiro success is claimed without credentials.
- `cargo fmt --all -- --check`, `cargo check --lib`, the Provider coverage/UI/
  Phase-0 audits, and `scripts/static-checks.sh` pass. Static checks include
  Clippy, 33 Node audit tests, Web typecheck, and 29 Web test files with 154
  passing tests.
- After an explicit binary build, `scripts/smoke/smoke-local.sh` passes health,
  version, embedded Web, offline setup, password/API-token authentication,
  Provider creation, and Share creation. The first cold invocation exceeded
  the script's 30-second readiness wait while compiling; the built binary
  started and passed the same smoke contract.
- `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` reports zero failures
  and `verificationState=blocked_inputs`. Its internal local-test blocker is
  expected because this negative readiness invocation deliberately skips tests;
  Router/Market/provider credentials and deployment evidence were not supplied.
  No real Kiro acceptance is claimed.

## 6. Cursor

### Reference Audit

- Highest-scoring reference: OmniRoute 9.0 at
  `918fba5e392ce8b137976349f035597196edc440`. The reviewed implementation is
  `open-sse/executors/cursor.ts`, `open-sse/executors/cursor/builtinToolBridge.ts`,
  `open-sse/executors/cursor/composer.ts`, and
  `open-sse/services/cursorSessionManager.ts`. It provides the strongest local
  evidence for the native Cursor Connect-RPC method, Composer model handling, declared-MCP
  bridging, explicit builtin rejection, and same-h2-stream tool-result resume.
  Its registry is keyed only by raw conversation ID and its documented
  cross-instance cold resume is intentionally not adopted because neither
  preserves this Server's exact Account/Share binding proof.
- Second-highest reference: 9router 7.8 at
  `15223724c3e1ad898e84ef6e0cc1686cbafc8290`. The reviewed implementation is
  `open-sse/executors/cursor.js`, `open-sse/services/cursorModels.js`, and
  `src/lib/oauth/services/cursor.js`, plus the Cursor import/OAuth routes and
  tests. It is useful for WorkOS/import compatibility, account metadata, model
  discovery, usage translation, and operational error samples; account
  fallback and active-account selection are excluded.
- The Server already had a materially deeper native driver than its original
  6.8 score suggested: h2/protobuf client-streaming, images, declared tools,
  tool-result park/resume, model modes, OAuth/API-key credential rails, and
  Claude/Chat/Responses/Gemini surfaces. This phase closes the identified
  isolation, deadline, builtin-tool, and exact-scope catalog gaps while keeping
  live acceptance separate.

### Verified Implementation

1. Introduce a typed `CursorSessionScope` and typed scoped keys. Its
   domain-separated digest covers App, Provider ID, Provider runtime revision,
   exact OAuth Account ID plus auth/token generations or API-key digest plus
   credential generation, exact Share ID, and normalized authenticated user
   email. Raw sensitive identifiers are never exposed as registry keys.
2. Key primary sessions by `(scope, conversation_id)` and response/tool indexes
   by `(scope, raw_id)`. The index value is the same typed primary key. Same raw
   conversation, response, and tool-call IDs in different scopes must coexist;
   lookup, replacement, close, expiry, and max-session eviction may affect only
   the exact scoped entry. A credential-generation change fails closed instead
   of deleting or resuming the old generation's stream.
3. Carry the request's authoritative `UsageLogContext.share_id` and normalized
   signed `user_email` into scope creation before resolving previous response or
   tool-call IDs. Missing Share/user values use explicit direct-invocation
   sentinels, never a wildcard.
4. Replace relative first-protocol-frame timing with an absolute
   first-business-output phase. The deadline starts when the initial request is
   sent, so response-header latency and partial bytes count. KV,
   request-context, heartbeat, tool lifecycle, and unknown control frames do not
   satisfy or extend it. A valid text/thinking/usage/tool-call/turn-end business
   event completes the phase; only then does the idle timeout apply.
5. Rearm a fresh first-business-output phase immediately after MCP tool results
   are submitted to a parked stream. The resumed phase uses the same exact
   scoped session and must time out independently when Cursor acknowledges only
   control traffic. Deadline failures surface as HTTP 504/error SSE and release
   Account, Share, and session leases.
6. Preserve the existing builtin matrix: read/write/edit/delete/list/glob/grep,
   diagnostics, shell/background shell, fetch, stdin, and MCP are bridged to a
   compatible declared tool when possible or answered with a protocol-correct
   explicit rejection. Add contract tests rather than introducing server-side
   filesystem or shell execution.
7. Extend identity/import, credential cache, model catalog/mode, entitlement,
   401/429 exact-binding, and all four response-surface fixtures. Keep Cursor
   Experimental and live-unverified until real OAuth, text, image, declared
   tool, builtin bridge, and park/resume acceptance is supplied.

8. Add first-class Gemini Cursor OAuth/API-key Profiles and Registry/UI Surface
   pairs. Gemini Provider classification recognizes both types, and signed
   Router Share `/v1beta/models` reuses the exact S2-encrypted API-key Provider
   scope. A successful empty public catalog is authoritative; configured/static
   aliases cannot survive it, and a distractor App/runtime/key scope cannot
   contribute models.

### Acceptance Invariants

- Two Shares, users, accounts, or credential generations may reuse every raw ID
  without replacement, discovery, close, or resume across their scopes.
- A same-scope continuation resumes only the original live h2 stream. Unknown,
  expired, or generation-mismatched tool results return a deterministic
  conflict/session-lost response and never cold-resume on another credential.
- Header wait, partial Connect bytes, and any number of control frames cannot
  extend the absolute first-business-output deadline. A resumed tool phase is
  subject to the same rule; idle timing begins only after a complete valid
  business frame in that phase.
- Claude Messages, OpenAI Chat, OpenAI Responses, and Gemini response shapes
  retain their tool IDs, reasoning/text ordering, usage, finish reason, and
  stream terminal contract.
- No Cursor path introduces account pools, rotation, cross-account retry,
  quota-based selection, or shadow credentials. A 401 may refresh and replay
  once only on the same authoritative binding; 429 never selects another
  Account.

### Verified Acceptance

- `cargo test --lib cursor` passes 188 tests. This includes scoped session and
  response/tool indexes, initial and post-tool absolute deadlines, protocol-level
  builtin rejection, credential/catalog isolation, same-account 401, bound 429,
  all four response emitters, and the signed Gemini Share catalog contract.
- Registry validation and Gemini classification tests pass. The Router catalog
  test creates a real `gemini.cursor_api_key` Provider through the domain write
  command, proves its committed S2 store contains no plaintext key, returns only
  exact-scope models, then accepts an authoritative empty catalog without any
  Cursor network request.
- Provider baseline and UI matrix audits pass after registering the new Surface
  pairs. Cursor remains `Experimental/live-unverified`; local fixtures do not
  claim real OAuth, inference, image, declared-tool, builtin, or park/resume
  success.

## 7. Kimi Code

### Verified Reference Evidence

- OmniRoute was re-audited at
  `918fba5e392ce8b137976349f035597196edc440` for Kimi Coding OAuth/import,
  K3 model variants, reasoning projection, and the distinction between the
  official Coding rail and Kimi Web/API rails. Only official Kimi Coding wire
  behavior was adopted; browser fallback, account scheduling, and API-key rail
  substitution are excluded.
- CLIProxyAPI was re-audited at
  `bd34ceca04209ef0460f4b05e3a1a047fb7fad2a` for the three application
  surfaces, authoritative model discovery, K3 effort normalization, signed
  thinking replay, refresh behavior, and error handling. Its account pool,
  rotation, and fallback mechanisms are excluded.

### Verified Implementation

- Kimi now has an App-aware typed wire contract. Claude Messages uses
  `/coding/v1/messages?beta=true`, Claude count_tokens uses
  `/coding/v1/messages/count_tokens?beta=true`, and Codex/Gemini bridges use
  `/coding/v1/chat/completions`; model discovery uses `/coding/v1/models`.
  Every request carries bearer auth plus the account-scoped Kimi device
  identity, and a 401 can refresh and replay only the same Account once.
- Model discovery is single-flight and scoped by App, Provider revision,
  runtime fingerprint, exact Account, `authIdentityGeneration`, and
  `tokenRefreshGeneration`. A successful empty upstream catalog is
  authoritative. Only the reviewed Kimi Coding allowlist is exposed; unknown
  and unreviewed models fail closed, and stale fallback is limited to a
  retryable failure in the identical scope.
- K3 accepts only canonical `low`, `high`, and `max` effort after normalizing
  reviewed aliases, defaults to `max`, and retains `thinking.keep=all`.
  Claude requests carry the owned `clear_thinking_20251015` edit; Chat history
  receives reasoning backfill only under the Kimi contract.
- Signed thinking replay is bounded and domain-separated by App, Provider
  revision/runtime, Account generation, Share, hashed signed user, session,
  and model family. It restores only a matching non-thinking assistant tool
  turn, uses CAS replace/delete, deletes rejected replay after upstream 400/422
  only when replay was actually applied, and blocks writes after Provider or
  Account generation drift. Streaming replay commits at `message_stop`, before
  upstream EOF, while incomplete/error/unknown streams never commit.
- Kimi Code remains a distinct managed Account/Provider type. No Kimi Web
  fallback exists, and Qoder-routed Kimi models cannot establish Kimi Code
  entitlement.

### Verified Acceptance

- `cargo test kimi --lib` passes 30 tests. They cover device flow identity,
  account header protection, authoritative model discovery, empty catalogs,
  exact three-App endpoints, reviewed model filtering, K3 reasoning, scoped
  non-stream/stream thinking replay, rejected-replay deletion, same-account
  401, and Account/Provider generation drift.
- `cargo check` passes. Coverage and regression documentation records Kimi as
  fixture-verified Native for Claude, Codex Responses/Chat, and Gemini while
  retaining the external gate.
- Real device login, token rotation, model catalog, inference, tools, images,
  quota, and failure recovery remain `live_pending`; no real Kimi credential
  was available and no live success is claimed.

## 8. Qoder / COSY

### Verified Reference Evidence

- TokenRouter was re-audited at
  `a63b6b6077738d7e2222f02ec050b70d3aeb3516` for typed Global/China accounts,
  device/PAT rails, OpenAPI/Gateway exchange, credential generations, model
  configuration, quota, protocol bridges, and session lifecycle. Its account
  pools, scheduling, balance routing, and cross-account recovery are excluded.
- 9router was re-audited at
  `15223724c3e1ad898e84ef6e0cc1686cbafc8290` for direct COSY signing, AES/WAF
  request encoding, live model config, streamed envelope decoding, quota paths,
  and site-specific endpoints. Browser impersonation and any credential
  selection outside the explicitly bound account are excluded.

### Verified Implementation

- `QoderCosy` is a first-class managed Provider/Account type with Claude,
  Codex, and Gemini Profiles. Global and China sites are explicit immutable
  account capabilities. Both expose bounded device login; Global additionally
  accepts an explicit `pt-*` PAT import. Public views expose only non-secret
  `site` and `credentialRail` metadata.
- OAuth and PAT layouts are mutually exclusive. Login/import requires a stable
  Qoder principal, preserves machine/site identity, and writes through the
  Account domain transaction. PAT exchange, access/job-token refresh, and
  quota retry can refresh only that exact account and are fenced by auth and
  token generations.
- COSY inference implements the reviewed fixed origins, signed headers,
  AES/WAF body contract, session creation/refresh, live model config, streamed
  envelope validation, embedded error handling, quota, and explicit auth,
  entitlement, and rate-limit taxonomy. Unknown or disabled models, stale
  sessions, post-terminal data, incomplete streams, and unsupported images fail
  closed.
- Claude Messages, OpenAI Responses/Chat, and Gemini requests canonicalize to
  one Qoder conversation contract while returning the caller's protocol. Text,
  reasoning, and declared tools are supported. Conversation, catalog, quota,
  and session state is scoped by site, credential rail, App, Provider revision
  and runtime, exact Account and generations, Share, signed user, session, and
  model.
- Pre-commit HTTP or embedded 401 recovery exchanges/refreshes and replays the
  same account once. Business output commits the response and disables replay;
  entitlement/rate-limit outcomes affect only the bound account. No path scans,
  rotates, or falls back to another account or Provider.

### Verified Acceptance

- `cargo test qoder --lib` passes 42 tests covering site/account invariants,
  fixed crypto vectors, device/PAT flows, quota, catalog/session scoping,
  three-App bridges, SSE terminal rules, same-account recovery, and generation
  drift. `cargo test --test api_contract qoder` passes the Web managed-auth
  site/state/cancel contract.
- The focused Web API/query/quota suite passes 26 tests, and typecheck passes.
  Provider registry, creation bridge, UI matrix, runtime command, and coverage
  audits include Qoder as a visible Experimental Provider.
- Real device/PAT login, exchange, discovery, inference, tools, quota, expiry,
  and recovery remain `live_pending`; no real Qoder credential was available
  and no live success is claimed.

## 9. API Key Coding Plans

### Verified Reference Evidence

- OmniRoute was re-audited at
  `ca23eed77cd19476141b3a39c74abee403203a68` for coding-plan catalogs, regional
  endpoint lockout, quota/reset parsing, plan labels, cache accounting, and
  flat-rate pricing semantics. Only single-credential protocol evidence is
  adopted; pool, combo, balance, and quota-spillover routing are excluded.
- TokenRouter was re-audited at
  `a63b6b6077738d7e2222f02ec050b70d3aeb3516` for Anthropic/OpenAI bridges,
  fixed endpoint capabilities, current GLM/MiniMax models, MiniMax thinking
  normalization, and defensive quota parsing. Its account/channel scheduler is
  excluded.

### Verified Implementation

- The Provider registry now carries a typed `codingPlan` contract for fixed
  inference origin/protocol/credential/auth, exact route paths, model catalog,
  quota adapter and credential roles, cache-token semantics, stream terminal,
  error/retry policy, and pricing evidence. Compilation rejects cross-App
  routes, auxiliary inference credentials, route drift, duplicate models, and
  invalid origins before a RuntimePlan can be committed.
- Sixteen Claude/Codex Profiles cover Kimi For Coding API Key, Zhipu GLM
  China/Global, MiniMax China/Global, Volcengine Coding Plan, and Xiaomi MiMo
  Token Plan China/Singapore. Every profile has a fixed origin and one explicit
  Provider credential source; configured endpoint overrides are ignored and no
  account pool or fallback exists.
- GLM catalogs include reviewed GLM-5.2/5.1/5/5 Turbo/4.7/4.7 Flash/4.6 and
  4.6V/4.5V/4.5/4.5 Air identifiers with bounded context/modalities. MiniMax
  includes M3, M2.7/highspeed, and M2.5/highspeed. MiMo keeps China and Singapore
  as separate fixed-origin families with their distinct Anthropic auth schemes.
- Kimi quota preserves authoritative primary windows, model-scoped weekly
  windows, and readable membership plans. Zhipu recognizes reviewed 5-hour and
  weekly shapes, including `unit=4, number=7`, while unknown explicit units fail
  closed. MiniMax and Volcengine parsers reject malformed/conflicting values;
  Volcengine request signing is locked to fixed vectors and reviewed actions.
  MiMo quota is explicitly `unavailable`, never inferred from local traffic.
- Quota cache identity includes App, Provider revision/runtime fingerprint,
  and credential generation. Concurrent refresh is
  single-flight; failed waiters receive the leader result, and stale data is
  exposed only inside the exact configured stale window. The Web display keeps
  `unavailable`, `unknown`, `stale`, and `supported` distinct and labels scoped
  windows such as `Weekly (kimi k3)`.
- MiniMax Anthropic requests normalize only
  `thinking.type=enabled` to `adaptive` for MiniMax M models on the two MiniMax
  coding-plan Profiles; budgets and vendor fields remain intact, and GLM/other
  Providers are unaffected. Flat-rate plans do not fabricate USD usage or
  authoritative upstream quota.

### Verified Acceptance

- `cargo test coding_plan --lib` covers contract compilation, route/path guards,
  cache isolation, Kimi/Zhipu/MiniMax/Volcengine parsing, fixed signing vectors,
  unavailable MiMo behavior, and profile-scoped MiniMax normalization.
- The current library inventory contains 28 `coding_plan`-filtered tests, all
  included in the passing full Rust suite.
- The coding-plan quota API contract, Provider creation bridge, registry
  inventory, coverage/baseline audits, Web typecheck, and quota component tests
  pass with all regional Profiles visible and redacted.
- No real API-key subscription credentials were available. Live inference,
  streaming/tools, quota/reset behavior, auth failure, and long-duration cache
  acceptance remain `live_pending`; fixture evidence is not reported as live
  success.

## Overall Review and Completion Gate

After all nine phases:

1. Review every state write, generation fence, cache/session/task namespace,
   endpoint override, and retry path for cross-account leakage.
2. Search for pool, rotation, random/weighted selection, balance-based routing,
   shadow accounts, and cross-account failover in all new code.
3. Run the full Rust and Web test suites, provider/UI audits, local smoke suite,
   offline release readiness, and every new fixture contract.
4. Reconcile implementation evidence with `docs/provider-coverage.md`,
   `UPSTREAM_IMPORT.md`, and `/data/projects/proxy/proxy.md`/`proxy.html`.
5. Change a score only where checked-in implementation and tests justify it;
   local fixtures never count as live acceptance.

### Final Review Outcome (2026-08-14)

- All nine phases retain exact Provider/Share/Account binding. Review of state
  writes, retry branches, cache/session/task keys, and endpoint resolution found
  no account-pool, weighted/random selection, balance routing, quota spillover,
  shadow-account, or cross-account fallback path in the new implementation.
- Review fixes are included in the verified tree: OpenAI Responses rejects a
  bare `[DONE]` immediately when no semantic terminal preceded it; Qoder is in
  the complete 17-Provider Account capability truth table; all 22 Provider
  types expose all three Adapter capability rows (66 total); Gemini Cursor and
  all 16 API-key coding-plan Profiles are present in the Web preset/icon maps;
  and test outbound clients use the shared HTTP transport constructor.
- `cargo test --no-fail-fast` passes 2,314 library tests, 121 API contract
  tests, the lease contract test, and doc tests. The nine focused inventories
  are Gemini 142, Grok 121, Antigravity 24, Copilot 48, Kiro 127, Cursor 153,
  Kimi 30, Qoder 42, and API-key coding plans 28.
- `scripts/static-checks.sh` passes rustfmt, Clippy, 34 Node audit tests, all
  Provider/UI/product-boundary audits, Web typecheck, and 32 Web test files
  with 164 tests. `cargo build --bin cc-switch-server` and the local smoke
  suite pass.
- `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` has zero failures and
  the expected `blocked_inputs` result. Real Router/Market/deployment and
  Provider credential inputs were not supplied, so every live gate remains
  explicit rather than being inferred from fixtures.
