# Provider Coverage

Provider types from: `/data/projects/cc-switch`
Presets from: `/data/projects/cc-switch`

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

## claude presets

| Name | providerType |
| --- | --- |
| Claude Official | `claude_oauth` |
| OpenAI OAuth | `codex_oauth` |
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

## codex presets

| Name | providerType |
| --- | --- |
| OpenAI OAuth | `codex_oauth` |
| Cursor API Key | `cursor_apikey` |
| Cursor OAuth | `cursor_oauth` |
| Ollama API Key | `ollama_cloud` |
| OpenRouter |  |
| Nvidia |  |
| DeepSeek(API Key) |  |

## gemini presets

| Name | providerType |
| --- | --- |
| Google Official | `google_gemini_oauth` |
| Antigravity OAuth | `antigravity_oauth` |
| Antigravity CLI (agy) | `agy_oauth` |
| OpenRouter |  |

## Server parity notes

### Claude/Codex model routing contract

- Native ownership is verified from Provider identity plus the official Anthropic/OpenAI endpoint; the display category is never an ownership signal. Native Claude and Codex providers persist `modelMapping.mode=passthrough` and retain the requested text model.
- Every non-native Claude/Codex Provider persists `modelMapping.mode=single` with one non-empty `upstreamModel`. The policy overrides catalogs, direct mappings, rules, role-model environment variables, Copilot preflight normalization, and vendor-specific Kiro/DeepSeek/Grok transforms.
- Provider load performs an idempotent migration. Explicit existing actual models are preserved, model values are inferred from app configuration when possible, and legacy Grok providers without an explicit mapping default to `grok-4.5`. Unresolvable historical custom providers remain loadable with a warning, while create/update/import rejects unresolved configuration.
- HTTP usage records preserve the requested model, record the final upstream model and source, and price by the final model. The routing policy applies to direct, Share, failover, health-check, HTTP, and Responses WebSocket text paths; Grok image/video routes are intentionally excluded.

### `kiro_oauth` (Kiro OAuth)

Server-native Kiro pass from `/data/projects/proxy/Kiro/Kiro.md` P0-P2 plus kiro.rs tool-call hardening (2026-07-13):

- OAuth/account storage: Builder ID and IdC device flow share AWS SSO OIDC registration, `issuerUrl` is persisted for IdC re-registration, and Google/GitHub Social login uses Kiro's server-safe device authorization/poll endpoints. Native refresh is selected dynamically by `authMethod` for Builder ID/IdC/Social/External IdP; OIDC refresh 401 can re-register the client and retry once.
- Imports: Kiro `credentials.json` can be pasted or read from the server host, and `ksk_` API keys are validated through `ListAvailableProfiles` before import. The account store recursively encrypts token/API-key/client-secret fields, including nested refresh responses.
- Proxy: Claude-only Kiro forwarding builds CodeWhisperer IDE requests by default and can use the CLI endpoint when account metadata sets `endpoint=cli`; requests add API_KEY/EXTERNAL_IDP `tokentype` when needed, default `profileArn` by auth method, and fall back to profileArn-derived region. EventStream parsing now validates prelude/message CRC and inline `<thinking>` content is split into Claude reasoning blocks.
- Tool-call hardening: top-level tool input schemas are forced to objects and unsupported combinators are stripped with object-field recovery. Non-stream tool JSON is buffered until `stop=true`; invalid or incomplete JSON returns a stable non-retryable 502 code. `TOOL_SCHEMA_INVALID` and `TOOL_USE_RESULT_MISMATCH` bypass retry/failover accounting, and `ksk_` values are masked before Kiro errors enter logs.
- Quota: `getUsageLimits` is available through the normal quota refresh path and refresh updates can backfill `kiroUsageLimits`.
- Real Kiro upstream validation remains an external gate: do not mark Kiro native acceptance complete until a real Kiro account has exercised Claude non-stream, stream, usage refresh, refresh-token rollover, and failover.

### `claude_oauth` (Claude Official)

Server-native Claude OAuth proxy parity pass from `/data/projects/proxy/Claude/Claude.md` through 2026-07-19:

- Proxy hot path: `?beta=true`, request-shape-driven `anthropic-beta` assembly (`claude-code-20250219`, `oauth-2025-04-20`, thinking/tools/computer/context/effort/1h-cache/explicit-1m only when allowed), Claude CLI header set, per-account stable stainless OS/arch profile, session metadata, billing/identity injection, thinking sampling normalization, preserve-order JSON, and one final `cch=` signature over the cleaned body. Client and body beta values use a fail-closed allowlist; unknown values are dropped without logging their raw token, and account extra headers cannot override the signed OAuth header contract.
- Retry hardening: Claude/Claude Auth/Claude OAuth streams buffer until the first complete non-error SSE data event (bounded at 64 KiB), so a split first `event:error` can record breaker outcome and retry before downstream commit. Send timeout/error, first-event read failure, and non-stream body-read failure use the same internal budget of at most three retries within 10s; automatic selection excludes providers already failed by the logical request, while explicit `x-cc-provider-id` and share binding remain pinned. Retry counters/body stages are no longer carried in client-controlled headers. Once any response data is committed, transport failure records the breaker signal and emits the protocol terminal error without replaying the request. Non-streaming 400 signature/thinking failures retain the reactive body stages for Claude OAuth only: thinking blocks become text, tool blocks can be downgraded on signature errors, and web_search history is stripped as the final fallback.
- Routes/usage/transform semantics: `/v1/messages/count_tokens` and `/claude/v1/messages/count_tokens` are available only through native `claude`, `claude_auth`, or `claude_oauth` providers; generation fields are removed, OAuth adds the token-counting beta and re-signs the final body, and the result is not recorded as generation usage. Normal generation usage remains four non-overlapping buckets. Cross-protocol SSE now buffers complete events across arbitrary chunks and keeps per-request Responses/Chat→Anthropic text/tool lifecycle, including parallel tools and packed argument done events.
- Operations hardening: the quota refresh loop first warm-refreshes due native OAuth tokens and isolates accounts after repeated `invalid_grant` failures, Claude OAuth accounts use per-account in-flight guards (default 8, provider/env configurable) and least-utilized selection while preserving failover queue tie-breaks, and non-streaming version-gate responses are rewritten into admin-facing guidance to bump `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA`. Downstream responses use an audited allowlist for `x-request-id`, `retry-after`, `x-should-retry`, and Anthropic rate-limit/priority/fast headers; cookies, server identity, and unreviewed headers are not copied. `/metrics` exports account concurrency, retry, breaker, warm-refresh, version-gate, and bootstrap result signals without account identity labels.
- OAuth web-paste/profile: `code#state` parsing, platform token endpoint first, platform User-Agent (`axios/1.13.6`). OAuth exchange performs a non-blocking `/api/claude_cli/bootstrap` lookup; quota refresh runs usage, profile, and bootstrap in parallel. The existing profile request now returns plan plus organization metadata and stores `billing_type` as `profile.billingSource` (`apple_subscription`, `stripe_subscription`, or a preserved unknown value) without deriving plan or expiry from it.
- Beta/session hardening: Claude OAuth accepts client/body beta values only from protocol-owned or audited compatibility sets, removes internal beta fields from serialized JSON, and exports bounded decision metrics. OAuth login sessions can be cancelled atomically before exchange, cancellation is idempotent and terminal, completed sessions retain the imported account id for idempotent multi-tab completion, and unknown states remain rejected. Cancellation is rejected after token exchange starts.
- Local callback uses `/api/accounts/login/callback`; Claude CLI callback route `/web-api/oauth/claude-cli/callback` is also registered, while a dedicated `127.0.0.1:54547` listener remains a deployment/productization choice.
- Evidence-gated exclusions: wire header casing/order and TLS/JA3 impersonation are deferred until captures show they are required; tool cloaking is not enabled without an observed OAuth tool-name block. The 54547 listener and MITM/DNS interception are not part of the headless server requirement. Skill, MCP, Tauri, session-manager, and Claude Desktop profile mutation remain outside the server product boundary.

### `codex_oauth` (OpenAI OAuth)

Server-native Codex/OpenAI OAuth proxy parity pass from `/data/projects/proxy/Codex/Codex.md` v2 P0-P2 plus TokenRouter account-candidate filtering through 2026-07-20:

- OAuth/account storage: CLI callback route `/web-api/oauth/openai-cli/callback`, serialized and cancellable/idempotent device polling, per-refresh-token singleflight/backoff, duplicate refresh-token rejection, immediate isolation on `refresh_token_reused`, and exclusive server token authority. Token fields are encrypted in `accounts.json`; OpenAI RS256 `id_token` values are verified against cached JWKS with issuer/audience/expiry checks before import or refresh. The Web UI can select only workspace/account IDs present in verified token organizations. The headless server does not live-read or write the host user's `~/.codex/auth.json`.
- Proxy headers/body: managed account requests finalize a paired official Codex identity (`originator`, configurable `version` defaulting to `0.144.1`, and User-Agent), inject the validated `chatgpt-account-id`, session/window headers, `reasoning.encrypted_content`, `prompt_cache_key`, and versioned instructions; invalid continuation `message` IDs are stripped without touching call IDs. GPT-5.6 Sol/Terra/Luna capabilities and reasoning gates are server-side registry data.
- Protocol/usage: Responses Lite `additional_tools`, custom/freeform history and response restoration, namespace flattening, `tool_search` downgrade/collision rejection, custom-tool stream completion, and strict wire zero fields are covered. OpenAI/Anthropic cache usage is normalized into fresh/read/write/output buckets, including nested `cache_write_tokens` and explicit zero values.
- Streaming/WS/images: Responses POST SSE keeps protocol conversion; Responses GET upgrades through WebSocket with a per-provider incident rollback toggle. SSE and WS `response.completed` events with empty output are rebuilt from prior `output_item.done`; Windows/Unix reset classification and big-frame `message_too_big` mapping are covered. `/v1/images/generations` dispatches to Grok OAuth media or the Codex image-generation bridge when enabled.
- Quota/subscription evidence: `/wham/usage.plan_type` is authoritative for the displayed plan. `/accounts/check` rejects expired or inactive candidates and uses exact matching for a verified workspace; `/subscriptions` is queried only for that verified workspace. Conflicting plans, untrusted workspace expiry, and past expiry contradicted by an available paid usage response are discarded, while sanitized resolution evidence is persisted for diagnostics. A discarded expiry is absent from both the auth summary and Share descriptor instead of being reported as expired.
- Rate limits/failover: 429 bodies parse `error.resets_in_seconds` and `error.resets_at`, write account `rateLimitedUntil`, and provider selection skips cooling-down Codex OAuth accounts; explicit provider selection returns 429 while cooling down.
- Client gate: inbound requests reject generic tool signatures while the final outbound header pass pairs official originator/User-Agent families and raises obsolete versions before every HTTP, WebSocket, and image request.
- TLS fingerprint: no Chrome/TLS impersonation is implemented in server; current stance is rustls direct TLS plus header/client gating. Real ChatGPT upstream smoke should revisit this only if upstream starts rejecting rustls traffic.

### `cursor_oauth` / `cursor_apikey` (Cursor AgentService)

Server-native Cursor OAuth/API key proxy parity pass from `/data/projects/proxy/Cursor/Cursor.md` P0-P2 (2026-07-09):

- OAuth/account storage: DeepControl PKCE + poll remains the browser login path; server now also imports Cursor IDE `state.vscdb` from the cc-switch-server host and falls back to cursor-agent `auth.json` across Linux/macOS/Windows (`CURSOR_AGENT_AUTH_PATH` can override). Imported IDE tokens preserve `cursorServiceMachineId`; agent auth imports are accepted without machine id. `CURSOR_STATE_DB_PATH` can override the IDE DB path; vscdb reads use an immutable SQLite URI to avoid live Cursor WAL locks; OAuth, local import, and profile enrichment derive account ids from the same WorkOS subject hash when available. Account token fields are covered by the shared encrypted `accounts.json` store.
- Profile enrichment: Cursor `/api/auth/me` uses the dashboard WorkOS session cookie shape (`WorkosCursorSessionToken=<workos_user_id>::<access_token>`) derived from the access-token JWT, not the generic `Authorization: Bearer` profile request. Token exchange/refresh, poll, and profile requests now share the Cursor browser-login User-Agent. Enrichment failure is non-fatal so access-token-only imports can still be used; when profile includes `sub`/`user_id`/`id`, it is used as the stable account id seed if tokens lack a subject.
- Proxy transport: Claude/Codex/Gemini Cursor providers use the native HTTP/2 Connect-RPC AgentService driver by default, with provider/env settings able to disable it during incident triage. The driver covers AgentService protobuf frames, cursor-agent CLI headers, KV/session handling, built-in tool rejection, declared tools, images, and Anthropic/OpenAI Chat/OpenAI Responses/Gemini response formatting. AgentService headers include W3C `traceparent`/`backend-traceparent`; timezone comes from `TZ`; client version is detected from local Cursor state with a 60-minute cache and falls back to `cli-2026.01.09-231024f`.
- Error/failover hardening: AgentService 429 responses now write account `rateLimitedUntil` from `Retry-After` or Cursor JSON reset hints, and provider selection skips cooling Cursor accounts. Non-2xx AgentService responses read up to 8KB of JSON error detail (`error`, `message`, `code`, `details[0].message`) so clients see actionable diagnostics instead of status-only 502s.
- Already present in the current tree: OmniRoute-derived `TOOL_COMMIT_DIRECTIVE`, CLI-minimal AgentService headers, 1MB image size limit plus private/link-local IP and `.internal`/`.local`/`.lan` host blocking, and per-account provider binding.
- Real Cursor upstream validation remains an external gate: do not mark live Cursor OAuth/API key proxy acceptance complete until a real Cursor account has exercised streaming, tool call/result continuation, images, and failover.

### `grok_oauth` (Grok/xAI OAuth)

Server-only capability from `/data/projects/proxy/Grok/Grok.md` P0-P2 (2026-07-09); not a desktop upstream provider coverage debt:

- OAuth/account storage: xAI public client id, PKCE, `plan=generic`, `referrer=cc-switch-server`, endpoint allowlist for `x.ai`/`*.x.ai`, JWT-derived profile fields, native refresh, and explicit `~/.grok/auth.json` import.
- Proxy headers/body: OpenAI Responses upstream contract, `Authorization: Bearer`, `x-grok-conv-id`, authoritative single-model routing with editable `grok-4.5` default, Responses field cleanup, reasoning effort model allowlist, tool allowlist, and `encrypted_content` shape guard.
- Media/WS: Grok images/videos routes forward to `api.x.ai/v1`; image edits translate common OpenAI multipart uploads to xAI JSON data URLs; Responses GET can bridge to `wss://api.x.ai/v1/responses`.
- Rate limits/failover: 401/403/429/5xx responses write account cooldown and provider selection skips cooling-down Grok accounts.
