# Provider Coverage

Generated from: `assets/contract/upstream-provider-source-baseline.json`
Server migration inventory: `assets/contract/server-provider-legacy-inventory.json`
Server ProviderType source: `src/domain/providers/model.rs`
Pinned upstream commit: `b1dee0153da94316fb50416c679a11f74cc66f14`

Note: server compatibility provider types are explicit cc-switch-server classifications for cc-switch presets that do not carry an upstream `providerType`.

## Provider Types

| ProviderType | Apps | Required | Present in source |
| --- | --- | --- | --- |
| `claude` | claude | yes | yes |
| `claude_auth` | claude | yes | yes |
| `claude_oauth` | claude | yes | yes |
| `codex` | codex | yes | yes |
| `codex_oauth` | claude, codex | yes | yes |
| `gemini` | gemini | yes | yes |
| `gemini_cli` | gemini, claude | yes | yes |
| `openrouter` | claude, codex, gemini | yes | yes |
| `github_copilot` | claude | yes | yes |
| `deepseek_account` | claude | yes | yes |
| `kiro_oauth` | claude | yes | yes |
| `cursor_oauth` | claude, codex | yes | yes |
| `cursor_apikey` | claude, codex | yes | yes |
| `antigravity_oauth` | claude, gemini | yes | yes |
| `agy_oauth` | claude, gemini | yes | yes |
| `ollama_cloud` | claude, codex | yes | yes |
| `aws_bedrock` | claude | no | NO |
| `nvidia` | claude, codex | no | NO |
| `deepseek_api` | claude, codex | no | NO |
| `grok_oauth` | claude, codex, gemini | no | NO |

## claude Server presets

| Name | providerType |
| --- | --- |
| Claude Official | `claude_oauth` |
| OpenAI OAuth | `codex_oauth` |
| Grok OAuth | `grok_oauth` |
| Kiro OAuth | `kiro_oauth` |
| Ollama API Key | `ollama_cloud` |
| Cursor OAuth | `cursor_oauth` |
| Cursor API Key | `cursor_apikey` |
| Antigravity OAuth | `antigravity_oauth` |
| Antigravity CLI (agy) | `agy_oauth` |
| GitHub Copilot | `github_copilot` |
| DeepSeek Official | `deepseek_account` |
| AWS Bedrock (AKSK) |  |
| AWS Bedrock (API Key) |  |
| OpenRouter |  |
| Nvidia |  |
| DeepSeek(API Key) |  |

## codex Server presets

| Name | providerType |
| --- | --- |
| OpenAI OAuth | `codex_oauth` |
| Grok OAuth | `grok_oauth` |
| Cursor API Key | `cursor_apikey` |
| Cursor OAuth | `cursor_oauth` |
| Ollama API Key | `ollama_cloud` |
| OpenRouter |  |
| Nvidia |  |
| DeepSeek(API Key) |  |

## gemini Server presets

| Name | providerType |
| --- | --- |
| Google Official | `google_gemini_oauth` |
| Antigravity OAuth | `antigravity_oauth` |
| Antigravity CLI (agy) | `agy_oauth` |
| Grok OAuth | `grok_oauth` |
| OpenRouter |  |

## Upstream app preset counts

| App | Count |
| --- | ---: |
| claude | 15 |
| codex | 7 |
| gemini | 4 |

## Universal recipes

| Name | providerType | Apps |
| --- | --- | --- |
| NewAPI | `newapi` | claude, codex, gemini |
| 自定义网关 | `custom_gateway` | claude, codex, gemini |

## Server implementation notes

### Claude/Codex model routing contract

- Typed Provider ownership is derived from immutable `profileId`; fixed Profiles ignore conflicting name, URL, category, and raw `meta.providerType` hints. Only S1/`legacy_compat` records retain endpoint/name heuristics. Native Claude and Codex Profiles persist `modelMapping.mode=passthrough` and retain the requested text model.
- Every non-native Claude/Codex Provider persists `modelMapping.mode=single` with one non-empty `upstreamModel`. The policy overrides catalogs, direct mappings, rules, role-model environment variables, Copilot preflight normalization, and vendor-specific Kiro/DeepSeek/Grok transforms.
- Provider load performs only an in-memory compatibility normalization and never rewrites `providers.json`. Explicit existing actual models are preserved, model values are inferred from legacy app configuration when possible, and legacy Grok providers without an explicit mapping default to `grok-4.5`. S1-to-S2 cutover is an explicit offline CLI action; unresolvable historical records block cutover rather than being guessed.
- HTTP usage records preserve the requested model, record the final upstream model and source, and attribute token usage to the final model. Direct requests use the selected current Provider and Share requests use their binding. Non-Claude retries and pinned Claude requests remain on that Provider; unpinned Claude Messages/count_tokens requests may use the bounded failover policy documented below. Grok image/video routes are intentionally excluded.

### Provider control plane and storage

- Rust `ProfileSpec` is the product identity authority, `DriverSpec` owns protocol operations, and each committed Provider compiles one canonical `RuntimePlan` shared by forwarding, manual test, and model discovery. Custom Profiles derive compatibility type deterministically from their explicit upstream protocol.
- Every Driver declares an `outboundIdentityPolicy`, and the compiled RuntimePlan applies it as the last header step after protocol authentication and managed-account overrides. Claude/Codex/Grok OAuth use their official CLI identity families; Kiro, Cursor, Copilot, and DeepSeek account drivers use their protocol-specific identities; Antigravity/agy use one background-refreshed client version and matching platform metadata; ordinary HTTP/API-key and Google OAuth drivers use `cc-switch-server/<version>`; Bedrock omits User-Agent; frozen legacy Profiles retain their existing contract.
- Only Custom HTTP Profiles can persist `customUserAgent`. Their empty value falls back to the Server identity, invalid header values are rejected, and `extraHeaders` cannot smuggle a second `User-Agent`. Preset Providers ignore historical values at runtime and clear a carried historical value on their next valid save; a new preset write containing a custom User-Agent is rejected.
- The same final identity pass covers normal HTTP forwarding, Claude prepared requests, Codex/Grok WebSocket handshakes, Codex HTTP fallback and image generation, Grok media, Provider network tests, model discovery, and scheduled Share health checks. Dedicated Kiro, Cursor AgentService, and DeepSeek transports continue to construct the same protocol-owned identities inside their native clients.
- Provider writes use `(app, providerId)`, expected revision, credential `keep/replace/clear`, and clone/validate/compile/seal/atomic-persist/swap ordering. Managed Profiles bind a concrete account identity; deleting a referenced Provider returns a conflict and never cascades into Share or Account stores.
- Fresh installations write guarded S2 `providers.json`; credentials are stored in XChaCha20-Poly1305 slot envelopes derived with HKDF from the shared root key. Existing S1 installations remain S1 until `cc-switch-server config migrate-provider-store --apply` is run while the Server is stopped.
- S2 protects an isolated `providers.json` or backup-file disclosure. `accounts.key`, the environment root key, the full data directory, or compromise of the Server OS user remains sufficient to decrypt credentials; this is not a hardware-backed secret boundary.
- S1/name/URL readers and `/api/provider-presets`, `/api/provider-matrix`, and `/api/provider-type` compatibility endpoints remain intentionally available. They cannot be removed until two stable bridge releases and at least 14 observation days are recorded in `provider-compatibility-window.json`; the current removal gate is not satisfied.

### `kiro_oauth` (Kiro OAuth)

Server-native Kiro pass from `/data/projects/proxy/Kiro/Kiro.md` P0-P2 plus kiro.rs tool-call hardening (2026-07-13):

- OAuth/account storage: Builder ID and IdC device flow share AWS SSO OIDC registration, `issuerUrl` is persisted for IdC re-registration, and Google/GitHub Social login uses Kiro's server-safe device authorization/poll endpoints. Native refresh is selected dynamically by `authMethod` for Builder ID/IdC/Social/External IdP; OIDC refresh 401 can re-register the client and retry once.
- Imports: Kiro `credentials.json` can be pasted or read from the server host, and `ksk_` API keys are validated through `ListAvailableProfiles` before import. The account store recursively encrypts token/API-key/client-secret fields, including nested refresh responses.
- Proxy: Claude-only Kiro forwarding builds CodeWhisperer IDE requests by default and can use the CLI endpoint when account metadata sets `endpoint=cli`; requests add API_KEY/EXTERNAL_IDP `tokentype` when needed, default `profileArn` by auth method, and fall back to profileArn-derived region. EventStream parsing now validates prelude/message CRC and inline `<thinking>` content is split into Claude reasoning blocks.
- Tool-call hardening: top-level tool input schemas are forced to objects and unsupported combinators are stripped with object-field recovery. Non-stream tool JSON is buffered until `stop=true`; invalid or incomplete JSON returns a stable non-retryable 502 code. `TOOL_SCHEMA_INVALID` and `TOOL_USE_RESULT_MISMATCH` bypass retry and Provider outcome accounting, and `ksk_` values are masked before Kiro errors enter logs.
- Quota: `getUsageLimits` is available through the normal quota refresh path and refresh updates can backfill `kiroUsageLimits`.
- Real Kiro upstream validation remains an external gate: do not mark Kiro native acceptance complete until a real Kiro account has exercised Claude non-stream, stream, usage refresh, refresh-token rollover, and rate-limit handling.

### `claude_oauth` (Claude Official)

Claude OAuth protocol evidence review from `/data/projects/proxy/Claude/Claude.md` through 2026-07-22, implemented independently in Server:

- Proxy hot path: legacy-compatible and typed Claude OAuth Providers share one prepared-request contract for network tests and real forwarding: managed-account refresh, `?beta=true`, request-shape-driven `anthropic-beta` assembly (`claude-code-20250219`, `oauth-2025-04-20`, thinking/tools/computer/context/effort/1h-cache/explicit-1m only when allowed), Claude CLI headers, per-account stable stainless OS/arch profile, session metadata, billing/identity injection, thinking sampling normalization, preserve-order JSON, and one final `cch=` signature over the cleaned body. Known Claude Code tools use canonical wire casing across declarations, `tool_choice`, and history, while streaming/non-streaming responses restore the request's declared casing; ambiguous case-insensitive declarations fail closed and custom names remain unchanged. Repeated client beta headers are merged through a fail-closed allowlist, unknown values are dropped without logging their raw token, repeated case-insensitive `[1m]` suffixes are removed before final signing, OAuth omits browser-only headers, and account extra headers cannot override the signed contract.
- Retry/failover hardening: Claude/Claude Auth/Claude OAuth streams buffer until the first complete non-error SSE data event (bounded at 64 KiB), so a split first `event:error` can record the Provider outcome before downstream commit. Unpinned direct Claude requests switch in Provider Store order after send timeout/error, first-event read failure, non-stream or 429 body-read failure, HTTP 429/529, or one forced OAuth refresh that remains 401. Candidates exclude prior failures and filter runtime readiness, relogin, account cooldown, count_tokens capability, and account concurrency under one budget of at most three retries within 10s. Share and explicit `x-cc-provider-id` requests stay pinned; OAuth signature/thinking/web-search body fallbacks also stay on the original Provider. Once any response data is committed, transport failure records the Provider outcome and emits the protocol terminal error without replaying the request.
- Routes/usage/transform semantics: `/v1/messages/count_tokens` and `/claude/v1/messages/count_tokens` are available only through native `claude`, `claude_auth`, or `claude_oauth` providers; generation fields are removed, OAuth adds the token-counting beta and re-signs the final body, and the result is not recorded as generation usage. Normal generation usage remains four non-overlapping buckets. Cross-protocol SSE now buffers complete events across arbitrary chunks and keeps per-request Responses/Chat→Anthropic text/tool lifecycle, including parallel tools and packed argument done events.
- Operations hardening: the quota refresh loop first warm-refreshes due native OAuth tokens and isolates accounts after repeated `invalid_grant` failures, Claude OAuth accounts use per-account in-flight guards (default 8, provider/env configurable) and least-utilized account selection inside the fixed Provider, and non-streaming version-gate responses are rewritten into admin-facing guidance to bump `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA`. Account identity generations now follow provider type plus the strongest stable principal rather than scopes, auth shape, email casing, or ordinary profile enrichment. Downstream responses use an audited allowlist for `x-request-id`, `retry-after`, `x-should-retry`, and Anthropic rate-limit/priority/fast headers; cookies, server identity, and unreviewed headers are not copied. `/metrics` exports retry/failover, Provider outcome, warm-refresh, version-gate, and bootstrap signals with bounded labels; account concurrency gauges remain keyed by provider type and internal account id.
- OAuth web-paste/profile: `code#state` parsing, platform token endpoint first, platform User-Agent (`axios/1.13.6`). OAuth exchange performs a non-blocking `/api/claude_cli/bootstrap` lookup; quota refresh runs usage, profile, and bootstrap in parallel. The existing profile request now returns plan plus organization metadata and stores `billing_type` as `profile.billingSource` (`apple_subscription`, `stripe_subscription`, or a preserved unknown value) without deriving plan or expiry from it.
- Beta/session hardening: Claude OAuth accepts client/body beta values only from protocol-owned or audited compatibility sets, removes internal beta fields from serialized JSON, and exports bounded decision metrics. OAuth login sessions can be cancelled atomically before exchange, cancellation is idempotent and terminal, completed sessions retain the imported account id for idempotent multi-tab completion, and unknown states remain rejected. Cancellation is rejected after token exchange starts.
- Local callback uses `/api/accounts/login/callback`; Claude CLI callback route `/web-api/oauth/claude-cli/callback` is also registered, while a dedicated `127.0.0.1:54547` listener remains a deployment/productization choice.
- Evidence-gated exclusions: wire header casing/order and TLS/JA3 impersonation are deferred until captures show they are required; tool cloaking is not enabled without an observed OAuth tool-name block. The 54547 listener and MITM/DNS interception are not part of the headless server requirement. Skill, MCP, Tauri, session-manager, and Claude Desktop profile mutation remain outside the server product boundary.

### `codex_oauth` (OpenAI OAuth)

Codex/OpenAI OAuth protocol evidence review from `/data/projects/proxy/pi`, `/data/projects/proxy/Codex/codex2api`, `/data/projects/proxy/Codex/Codex.md` v2 P0-P2, and TokenRouter account-candidate filtering through 2026-07-22, implemented independently in Server. `pi` is used as the official OAuth/client-behavior reference, not as the multi-account server architecture template:

- OAuth/account storage: Device OAuth and official CLI PKCE OAuth share the server login state machine. For the configured remote HTTPS Client URL, CLI OAuth preserves `http://localhost:1455/auth/callback`; after the browser's local redirect fails, the administrator submits the complete callback URL to the originating, principal-bound login session. The Server requires a signed Router ingress and same-origin Client URL request, then validates the exact callback origin/path, state and expiry before exchange. Every supported device flow binds start/poll/cancel to the authenticated administrator principal for the device-code lifetime; Codex polling is serialized, cancellable and idempotent. Refresh uses per-token singleflight/backoff, duplicate refresh tokens are rejected, and `refresh_token_reused` immediately isolates the account. Token fields are encrypted in `accounts.json`, while control-plane responses expose only credential-presence booleans and sanitized runtime state; no plaintext account credential export endpoint is exposed.
- OpenAI trust boundary: both ID and access JWTs use cached OpenAI JWKS with RS256, issuer, audience, expiry/nbf and `kid` rotation checks. One canonical extractor reads the literal `https://api.openai.com/auth` object (plus explicit legacy shapes), keeps user subject separate from `chatgpt_account_id`, continues from an empty ID-token identity to the verified access token, rejects conflicts, and requires both a non-empty subject and workspace. New local account record IDs are a stable SHA-256-derived subject ID; workspace remains only the upstream `chatgpt-account-id` identity. Existing records with the same verified subject are reused atomically, and refresh fails closed if a previously verified account returns a different subject. Workspace selection and the outbound header consume only verified claims or authenticated discovery provenance. The executable cases live in `assets/contract/openai-oauth-protocol.json`.
- Endpoint and binding policy: managed Codex OAuth authorization, token, quota and inference endpoints are fixed to the audited official origins; provider/user endpoint overrides cannot receive OAuth credentials. Every managed OAuth Provider must bind a concrete compatible account. The headless server does not live-read or write the host user's `~/.codex/auth.json`.
- Proxy headers/body: managed account requests finalize a paired official Codex identity (`originator`, configurable `version` defaulting to `0.144.1`, and User-Agent), inject the validated `chatgpt-account-id`, session/window headers, `reasoning.encrypted_content`, `prompt_cache_key`, and versioned instructions; invalid continuation `message` IDs are stripped without touching call IDs. GPT-5.6 Sol/Terra/Luna capabilities and reasoning gates are server-side registry data.
- Protocol/usage: Responses Lite `additional_tools`, custom/freeform history and response restoration, namespace flattening, `tool_search` downgrade/collision rejection, custom-tool stream completion, and strict wire zero fields are covered. OpenAI/Anthropic cache usage is normalized into fresh/read/write/output buckets, including nested `cache_write_tokens` and explicit zero values.
- Streaming/WS/images: Responses POST SSE keeps protocol conversion; Responses GET upgrades through WebSocket with a per-provider incident rollback toggle. Codex WS connections use a bounded pool keyed by process, Provider/runtime, session, upstream URL and credential/workspace headers, with capacity, idle TTL and max-age eviction. Connect/5xx handshake/send/read/stale-cache/1009 failures may replay the active `response.create` through HTTP/SSE only before the first downstream business event; the configured stream first-event timeout (default 120 seconds) covers request send, response headers, and that first valid event without being extended by SSE comments or partial bytes. After the first event, the idle timeout (default 300 seconds) only terminates the stream. Handshake 4xx and committed responses never trigger transport replay. HTTP fallback keeps the same execution/account/workspace/concurrency lease, supports flat and nested request frames, bounds one SSE event to 128 MiB and rematerializes auth after one same-account 401 refresh. SSE and WS `response.completed` events with empty output are rebuilt from prior `output_item.done`; Windows/Unix reset classification and big-frame `message_too_big` mapping are covered. `/v1/images/generations` dispatches to Grok OAuth media or the Codex image-generation bridge when enabled.
- Quota/subscription evidence: `/wham/usage.plan_type` is authoritative for the displayed plan. `/accounts/check` rejects expired or inactive candidates and uses exact matching for a verified workspace; `/subscriptions` is queried only for that verified workspace. Conflicting plans, untrusted workspace expiry, and past expiry contradicted by an available paid usage response are discarded, while sanitized resolution evidence is persisted for diagnostics. A discarded expiry is absent from both the auth summary and Share descriptor instead of being reported as expired.
- Rate limits/account dispatch: Codex 429 bodies parse `error.resets_in_seconds` and `error.resets_at`; generic managed-account handling also honors `Retry-After`, bounds cooldowns, and writes only account `rateLimitedUntil`. Unpinned managed requests choose compatible Provider/account candidates by current/max concurrency ratio, then stable rendezvous affinity for equal load; active cooldown, relogin, explicit exhaustion and saturated accounts are excluded. HTTP, SSE, Images and WS handshake/fallback receive one same-account forced refresh on 401 before cooldown/failover. Explicit Provider and Share bindings remain pinned and return their own error rather than silently switching identity.
- Client gate: inbound requests reject generic tool signatures while the final outbound header pass pairs official originator/User-Agent families and raises obsolete versions before every HTTP, WebSocket, and image request.
- TLS fingerprint: no Chrome/TLS impersonation is implemented in server; current stance is rustls direct TLS plus header/client gating. Real ChatGPT upstream smoke should revisit this only if upstream starts rejecting rustls traffic.

### `cursor_oauth` / `cursor_apikey` (Cursor AgentService)

Cursor OAuth/API key protocol evidence review from `/data/projects/proxy/Cursor/Cursor.md` P0-P2 (2026-07-09), implemented independently in Server:

- OAuth/account storage: DeepControl PKCE + poll remains the browser login path; server now also imports Cursor IDE `state.vscdb` from the cc-switch-server host and falls back to cursor-agent `auth.json` across Linux/macOS/Windows (`CURSOR_AGENT_AUTH_PATH` can override). Imported IDE tokens preserve `cursorServiceMachineId`; agent auth imports are accepted without machine id. `CURSOR_STATE_DB_PATH` can override the IDE DB path; vscdb reads use an immutable SQLite URI to avoid live Cursor WAL locks; OAuth, local import, and profile enrichment derive account ids from the same WorkOS subject hash when available. Account token fields are covered by the shared encrypted `accounts.json` store.
- Profile enrichment: Cursor `/api/auth/me` uses the dashboard WorkOS session cookie shape (`WorkosCursorSessionToken=<workos_user_id>::<access_token>`) derived from the access-token JWT, not the generic `Authorization: Bearer` profile request. Token exchange/refresh, poll, and profile requests now share the Cursor browser-login User-Agent. Enrichment failure is non-fatal so access-token-only imports can still be used; when profile includes `sub`/`user_id`/`id`, it is used as the stable account id seed if tokens lack a subject.
- Proxy transport: Claude/Codex/Gemini Cursor providers use the native HTTP/2 Connect-RPC AgentService driver by default, with provider/env settings able to disable it during incident triage. The driver covers AgentService protobuf frames, cursor-agent CLI headers, KV/session handling, built-in tool rejection, declared tools, images, and Anthropic/OpenAI Chat/OpenAI Responses/Gemini response formatting. AgentService headers include W3C `traceparent`/`backend-traceparent`; timezone comes from `TZ`; client version is detected from local Cursor state with a 60-minute cache and falls back to `cli-2026.01.09-231024f`.
- Rate-limit hardening: AgentService 429 responses now write account `rateLimitedUntil` from `Retry-After` or Cursor JSON reset hints, and account selection inside the fixed Cursor Provider skips cooling accounts. Non-2xx AgentService responses read up to 8KB of JSON error detail (`error`, `message`, `code`, `details[0].message`) so clients see actionable diagnostics instead of status-only 502s.
- Already present in the current tree: OmniRoute-derived `TOOL_COMMIT_DIRECTIVE`, CLI-minimal AgentService headers, 1MB image size limit plus private/link-local IP and `.internal`/`.local`/`.lan` host blocking, and per-account provider binding.
- Real Cursor upstream validation remains an external gate: do not mark live Cursor OAuth/API key proxy acceptance complete until a real Cursor account has exercised streaming, tool call/result continuation, images, and rate-limit/cooldown behavior.

### `grok_oauth` (Grok/xAI OAuth)

Server-owned capability based on protocol evidence from `/data/projects/proxy/Grok/Grok.md` P0-P2 (2026-07-09); it is not part of the external Provider baseline:

- OAuth/account storage: xAI public client id, PKCE, `plan=generic`, `referrer=cc-switch-server`, endpoint allowlist for `x.ai`/`*.x.ai`, JWT-derived profile fields, native refresh, and explicit `~/.grok/auth.json` import.
- Proxy headers/body: OpenAI Responses upstream contract, `Authorization: Bearer`, `x-grok-conv-id`, authoritative single-model routing with editable `grok-4.5` default, Responses field cleanup, reasoning effort model allowlist, tool allowlist, and `encrypted_content` shape guard.
- Media/WS: Grok images/videos routes forward to `api.x.ai/v1`; image edits translate common OpenAI multipart uploads to xAI JSON data URLs; Responses GET can bridge to `wss://api.x.ai/v1/responses`.
- Rate limits/account cooldown: 401/403/429/5xx responses write account cooldown and account selection inside the fixed Grok Provider skips cooling-down accounts.
- Quota/subscription expiry: weekly and monthly billing responses remain quota evidence only; `currentPeriod.end` and `billingPeriodEnd` are never treated as the payment/subscription expiry. An explicit expiry on an active subscription remains authoritative when available. Otherwise each Grok account can store a manual next-payment expiry, which survives OAuth refresh and is synchronized to provider and Share metadata without affecting credential validity or proxy scheduling.
