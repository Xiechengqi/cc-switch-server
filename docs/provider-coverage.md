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

### `kiro_oauth` (Kiro OAuth)

Server-native Kiro pass from `/data/projects/proxy/Kiro/Kiro.md` P0-P2 plus kiro.rs tool-call hardening (2026-07-13):

- OAuth/account storage: Builder ID and IdC device flow share AWS SSO OIDC registration, `issuerUrl` is persisted for IdC re-registration, and Google/GitHub Social login uses Kiro's server-safe device authorization/poll endpoints. Native refresh is selected dynamically by `authMethod` for Builder ID/IdC/Social/External IdP; OIDC refresh 401 can re-register the client and retry once.
- Imports: Kiro `credentials.json` can be pasted or read from the server host, and `ksk_` API keys are validated through `ListAvailableProfiles` before import. The account store recursively encrypts token/API-key/client-secret fields, including nested refresh responses.
- Proxy: Claude-only Kiro forwarding builds CodeWhisperer IDE requests by default and can use the CLI endpoint when account metadata sets `endpoint=cli`; requests add API_KEY/EXTERNAL_IDP `tokentype` when needed, default `profileArn` by auth method, and fall back to profileArn-derived region. EventStream parsing now validates prelude/message CRC and inline `<thinking>` content is split into Claude reasoning blocks.
- Tool-call hardening: top-level tool input schemas are forced to objects and unsupported combinators are stripped with object-field recovery. Non-stream tool JSON is buffered until `stop=true`; invalid or incomplete JSON returns a stable non-retryable 502 code. `TOOL_SCHEMA_INVALID` and `TOOL_USE_RESULT_MISMATCH` bypass retry/failover accounting, and `ksk_` values are masked before Kiro errors enter logs.
- Quota: `getUsageLimits` is available through the normal quota refresh path and refresh updates can backfill `kiroUsageLimits`.
- Real Kiro upstream validation remains an external gate: do not mark Kiro native acceptance complete until a real Kiro account has exercised Claude non-stream, stream, usage refresh, refresh-token rollover, and failover.

### `claude_oauth` (Claude Official)

Server-native Claude OAuth proxy parity pass from `/data/projects/proxy/Claude/CLAUDE.md` P0/P1 plus second/third/fourth/fifth/sixth-round hardening (2026-07-10):

- Proxy hot path: `?beta=true`, request-shape-driven `anthropic-beta` assembly (`claude-code-20250219`, `oauth-2025-04-20`, plus thinking/tool/computer-use/context-management betas only when needed), Claude CLI header set, per-account stable stainless OS/arch profile, stream-sensitive `x-stainless-timeout`, first-user-text-seeded `x-claude-code-session-id`, `metadata.user_id`, billing/identity `system` injection with billing block dedupe and optional 1h cache TTL, default `tools: []`, `max_tokens=128000`, `temperature=1`, and thinking `context_management`, preserve-order JSON serialization, `cch=` body signing, env-overridable CCH seed, and `cc_entrypoint=cli` CCH default (`src/proxy/claude_oauth.rs`, `src/domain/claude_cli.rs`).
- Retry hardening: Claude OAuth streams pre-read the first upstream chunk; first-chunk `event:error` records breaker outcome and retries within a 3-attempt/10s budget. Non-streaming 400 signature/thinking failures trigger reactive body retry stages: thinking blocks become text, tool blocks can be downgraded to text on tool-signature errors, and web_search history blocks are stripped as a final fallback.
- Operations hardening: the quota refresh loop first warm-refreshes due native OAuth tokens and isolates accounts after repeated `invalid_grant` failures, Claude OAuth accounts use per-account in-flight guards (default 8, provider/env configurable) and least-utilized selection while preserving failover queue tie-breaks, non-streaming version-gate responses are rewritten into admin-facing guidance to bump `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA`, upstream `x-request-id` is passed through for support correlation, and `/metrics` exports account concurrency, retry, breaker, warm-refresh, and version-gate signals.
- OAuth web-paste: `code#state` parsing, platform token endpoint first, platform User-Agent (`axios/1.13.6`).
- Local callback uses `/api/accounts/login/callback`; Claude CLI callback route `/web-api/oauth/claude-cli/callback` is also registered, while a dedicated `127.0.0.1:54547` listener remains a deployment/productization choice.

### `codex_oauth` (OpenAI OAuth)

Server-native Codex/OpenAI OAuth proxy parity pass from `/data/projects/proxy/Codex/Codex.md` P0-P2 (2026-07-09):

- OAuth/account storage: CLI callback route `/web-api/oauth/openai-cli/callback`, per-account refresh serialization, token fields encrypted in `accounts.json` with `accounts.key` included in backup targets, and JWT/profile-derived fields `chatgpt_account_id`, `chatgptAccountId`, `planType`, `poid`, `organizations`, `subscription.expiresAt`.
- Proxy headers/body: managed account requests inject `chatgpt-account-id`, `originator=codex_cli_rs`, `version=0.125.0`, Codex CLI User-Agent, session/window headers, `reasoning.encrypted_content`, `prompt_cache_key`, and versioned Codex instructions templates; ordinary `/v1/responses` requests carrying `input[].type=compaction_trigger` are promoted to `/responses/compact`.
- Streaming/WS/images: Responses POST SSE keeps existing protocol conversion; Responses GET upgrades through WebSocket to `wss://chatgpt.com/backend-api/codex/responses` with `openai-beta: responses_websockets=2026-02-06` and maps big-frame failures to `message_too_big`; stream `response.completed` events with missing/empty `response.output` are patched from prior `response.output_item.done` events; unnamed pending function-call starts are delayed until `done` supplies the tool name; `/v1/images/generations` dispatches to Grok OAuth media or Codex OAuth image-generation bridge when enabled.
- Rate limits/failover: 429 bodies parse `error.resets_in_seconds` and `error.resets_at`, write account `rateLimitedUntil`, and provider selection skips cooling-down Codex OAuth accounts; explicit provider selection returns 429 while cooling down.
- Client gate: Codex OAuth upstream calls reject generic tool signatures when `originator` is present and require a Codex-compatible originator plus official UA engine shape. Requests without `originator` remain allowed except obvious generic tools, to avoid breaking older official clients.
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
- Proxy headers/body: OpenAI Responses upstream contract, `Authorization: Bearer`, `x-grok-conv-id`, model mapping to `grok-4.3`, Responses field cleanup, reasoning effort model allowlist, tool allowlist, and `encrypted_content` shape guard.
- Media/WS: Grok images/videos routes forward to `api.x.ai/v1`; image edits translate common OpenAI multipart uploads to xAI JSON data URLs; Responses GET can bridge to `wss://api.x.ai/v1/responses`.
- Rate limits/failover: 401/403/429/5xx responses write account cooldown and provider selection skips cooling-down Grok accounts.
