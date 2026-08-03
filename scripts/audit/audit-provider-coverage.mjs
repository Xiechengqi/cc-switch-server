#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

import {
  assertRequiredProviderProfileCoverage,
  requiredProviderTypes,
  serverCompatibilityProviderTypes,
} from "./provider-profile-coverage.mjs";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const checkMode = process.argv.includes("--check");
const upstreamBaselinePath = path.join(
  repoRoot,
  "assets/contract/upstream-provider-source-baseline.json",
);
const upstreamBaseline = JSON.parse(
  fs.readFileSync(upstreamBaselinePath, "utf8"),
);
const serverLegacyInventoryPath = path.join(
  repoRoot,
  "assets/contract/server-provider-legacy-inventory.json",
);
const serverLegacyInventory = JSON.parse(
  fs.readFileSync(serverLegacyInventoryPath, "utf8"),
);
const providerRegistryPath = path.join(
  repoRoot,
  "assets/contract/provider-registry.json",
);
const providerRegistry = JSON.parse(
  fs.readFileSync(providerRegistryPath, "utf8"),
);

const providerTypeMetadata = new Map(
  [...requiredProviderTypes, ...serverCompatibilityProviderTypes].map(
    ([id, label, apps], index) => [
      id,
      { label, apps, required: index < requiredProviderTypes.length },
    ],
  ),
);

function buildCoverage() {
  const sourceProviderTypes = new Set(
    upstreamBaseline.providerTypes.map((providerType) => providerType.id),
  );
  const providerTypes = serverLegacyInventory.providerTypes.map(
    ({ variant, id }) => {
      const metadata = providerTypeMetadata.get(id);
      if (!metadata) {
        throw new Error(
          `Server ProviderType ${id} is missing reviewed coverage metadata`,
        );
      }
      return {
        variant,
        id,
        label: metadata.label,
        apps: metadata.apps,
        required: metadata.required,
        presentInSource: sourceProviderTypes.has(id),
        presentInServer: true,
      };
    },
  );

  return {
    generatedFrom: {
      baseline: path.relative(repoRoot, upstreamBaselinePath),
      serverLegacyInventory: path.relative(repoRoot, serverLegacyInventoryPath),
      serverProviderTypes: serverLegacyInventory.providerTypeSource,
      upstreamCommit: upstreamBaseline.upstream.commit,
    },
    providerTypes,
    upstreamPresets: upstreamBaseline.appPresets,
    presets: Object.fromEntries(
      Object.entries(serverLegacyInventory.presets).map(([app, presets]) => [
        app,
        presets.map((preset) => ({
          name: preset.name,
          providerType: preset.providerType,
          apiFormat: preset.apiFormat,
          baseUrl: preset.baseUrl,
          defaultModel: preset.defaultModel,
          sourceIndex: preset.sourceIndex,
        })),
      ]),
    ),
    universalRecipes: upstreamBaseline.universalRecipes,
  };
}

function providerFixture(app, preset) {
  const settingsConfig = {};
  if (preset.baseUrl) {
    settingsConfig.env = {};
    if (app === "gemini") {
      settingsConfig.env.GOOGLE_GEMINI_BASE_URL = preset.baseUrl;
    } else if (app === "codex") {
      settingsConfig.env.OPENAI_BASE_URL = preset.baseUrl;
    } else if (app === "claude") {
      settingsConfig.env.ANTHROPIC_BASE_URL = preset.baseUrl;
    }
  }

  const meta = {};
  if (preset.providerType) meta.providerType = preset.providerType;
  if (preset.apiFormat) meta.apiFormat = preset.apiFormat;

  return {
    app,
    name: preset.name,
    expectedProviderType: expectedProviderType(app, preset),
    provider: {
      id: `${app}:${preset.name}`,
      name: preset.name,
      settingsConfig,
      meta: Object.keys(meta).length > 0 ? meta : null,
    },
  };
}

function expectedProviderType(app, preset) {
  if (app === "claude") {
    if (preset.providerType === "google_gemini_oauth") return "gemini_cli";
    if (preset.providerType) return preset.providerType;
    if (preset.baseUrl?.includes("openrouter.ai")) return "openrouter";
    if (preset.baseUrl?.includes("bedrock-runtime.")) return "aws_bedrock";
    if (preset.baseUrl?.includes("integrate.api.nvidia.com")) return "nvidia";
    if (preset.baseUrl?.includes("api.deepseek.com")) return "deepseek_api";
    return "claude";
  }

  if (app === "codex") {
    if (
      [
        "codex_oauth",
        "grok_oauth",
        "cursor_oauth",
        "cursor_apikey",
        "ollama_cloud",
      ].includes(preset.providerType)
    ) {
      return preset.providerType;
    }
    if (preset.baseUrl?.includes("openrouter.ai")) return "openrouter";
    if (preset.baseUrl?.includes("integrate.api.nvidia.com")) return "nvidia";
    if (preset.baseUrl?.includes("api.deepseek.com")) return "deepseek_api";
    return "codex";
  }

  if (app === "gemini") {
    if (preset.providerType === "google_gemini_oauth") return "gemini_cli";
    if (
      ["antigravity_oauth", "agy_oauth", "grok_oauth"].includes(
        preset.providerType,
      )
    ) {
      return preset.providerType;
    }
    if (preset.baseUrl?.includes("openrouter.ai")) return "openrouter";
    return "gemini";
  }

  return null;
}

function toMarkdown(coverage) {
  const lines = [];
  lines.push("# Provider Coverage");
  lines.push("");
  lines.push(`Generated from: \`${coverage.generatedFrom.baseline}\``);
  lines.push(
    `Server migration inventory: \`${coverage.generatedFrom.serverLegacyInventory}\``,
  );
  lines.push(
    `Server ProviderType source: \`${coverage.generatedFrom.serverProviderTypes.path}\``,
  );
  lines.push(
    `Pinned upstream commit: \`${coverage.generatedFrom.upstreamCommit}\``,
  );
  lines.push("");
  lines.push(
    "Note: server compatibility provider types are explicit cc-switch-server classifications for cc-switch presets that do not carry an upstream `providerType`.",
  );
  lines.push("");
  lines.push("## Provider Types");
  lines.push("");
  lines.push("| ProviderType | Apps | Required | Present in source |");
  lines.push("| --- | --- | --- | --- |");
  for (const item of coverage.providerTypes) {
    lines.push(
      `| \`${item.id}\` | ${item.apps.join(", ")} | ${item.required ? "yes" : "no"} | ${item.presentInSource ? "yes" : "NO"} |`,
    );
  }
  lines.push("");
  for (const key of ["claude", "codex", "gemini"]) {
    lines.push(`## ${key} Server presets`);
    lines.push("");
    lines.push("| Name | providerType |");
    lines.push("| --- | --- |");
    for (const preset of coverage.presets[key]) {
      lines.push(
        `| ${preset.name} | ${preset.providerType ? `\`${preset.providerType}\`` : ""} |`,
      );
    }
    lines.push("");
  }
  lines.push("## Upstream app preset counts");
  lines.push("");
  lines.push("| App | Count |");
  lines.push("| --- | ---: |");
  for (const key of ["claude", "codex", "gemini"]) {
    lines.push(`| ${key} | ${coverage.upstreamPresets[key].length} |`);
  }
  lines.push("");
  lines.push("## Universal recipes");
  lines.push("");
  lines.push("| Name | providerType | Apps |");
  lines.push("| --- | --- | --- |");
  for (const recipe of coverage.universalRecipes) {
    const apps = Object.entries(recipe.defaultApps)
      .filter(([, enabled]) => enabled)
      .map(([app]) => app)
      .join(", ");
    lines.push(`| ${recipe.name} | \`${recipe.providerType}\` | ${apps} |`);
  }
  lines.push("");
  lines.push(...serverEvidenceNotes());
  return `${lines.join("\n").trimEnd()}\n`;
}

function serverEvidenceNotes() {
  return [
    "## Server implementation notes",
    "",
    "### Claude/Codex/Gemini model routing contract",
    "",
    "- Typed Provider ownership is derived from immutable `profileId`; fixed Profiles ignore conflicting name, URL, category, and raw `meta.providerType` hints. Only S1/`legacy_compat` records retain endpoint/name heuristics. Native official Claude, OpenAI, and Google Profiles are locked to `modelMapping.mode=passthrough` and retain the requested text model.",
    "- Non-native and Custom Profiles default to `modelMapping.mode=single` with the Registry-owned non-empty `defaultUpstreamModel`, but may be switched explicitly to `passthrough`. Single mode overrides catalogs, direct mappings, rules, role-model environment variables, Copilot preflight normalization, and vendor-specific Kiro/DeepSeek/Grok transforms. Passthrough retains the requested model while still allowing protocol adapters to normalize audited aliases.",
    "- Provider load performs only an in-memory compatibility normalization and never rewrites `providers.json`. An allowed explicit mode and actual model are preserved; missing non-native mappings use the Profile default, while typed official records are repaired to passthrough. Legacy model values are inferred from app configuration when possible, and legacy Grok providers without an explicit mapping default to `grok-4.5`. S1-to-S2 cutover is an explicit offline CLI action; unresolvable historical records block cutover rather than being guessed.",
    "- HTTP usage records preserve the requested model, record the final upstream model and source, and attribute token usage to the final model. Direct Claude and Codex requests use only the Surface resolved by the URL Route Key and that Provider Bundle's bound account. Codex OAuth active-account selection is account-center-only and does not affect routing. Neither route enters cross-Provider or cross-account failover; Share requests additionally retain their immutable binding. Other routes retain their documented routing policies. Grok image/video routes are intentionally excluded.",
    "",
    "### Provider control plane and storage",
    "",
    "- Rust `ProfileSpec` is the product identity authority, `DriverSpec` owns protocol operations, and each committed Provider compiles one canonical `RuntimePlan` shared by forwarding, manual test, and model discovery. Custom Profiles derive compatibility type deterministically from their explicit upstream protocol.",
    "- Every Driver declares an `outboundIdentityPolicy`, and the compiled RuntimePlan applies it as the last header step after protocol authentication and managed-account overrides. Claude/Codex/Grok and Google Code Assist OAuth use their official CLI identity families; Kiro, Cursor, Copilot, and DeepSeek account drivers use their protocol-specific identities; Antigravity/agy use one background-refreshed client version and matching platform metadata; ordinary HTTP/API-key drivers use `cc-switch-server/<version>`; Bedrock omits User-Agent; frozen legacy Profiles retain their existing contract.",
    "- Only Custom HTTP Profiles can persist `customUserAgent`. Their empty value falls back to the Server identity, invalid header values are rejected, and `extraHeaders` cannot smuggle a second `User-Agent`. Preset Providers ignore historical values at runtime and clear a carried historical value on their next valid save; a new preset write containing a custom User-Agent is rejected.",
    "- The same final identity pass covers normal HTTP forwarding, Claude prepared requests, Codex/Grok WebSocket handshakes, Codex HTTP fallback and image generation, Grok media, Provider network tests, model discovery, and scheduled Share health checks. Dedicated Kiro, Cursor AgentService, and DeepSeek transports continue to construct the same protocol-owned identities inside their native clients.",
    "- Provider writes use `(app, providerId)`, expected revision, credential `keep/replace/clear`, and clone/validate/compile/seal/atomic-persist/swap ordering. Managed Profiles bind a concrete account identity; deleting a referenced Provider returns a conflict and never cascades into Share or Account stores.",
    "- Fresh installations write guarded S2 `providers.json`; credentials are stored in XChaCha20-Poly1305 slot envelopes derived with HKDF from the shared root key. Existing S1 installations remain S1 until `cc-switch-server config migrate-provider-store --apply` is run while the Server is stopped.",
    "- S2 protects an isolated `providers.json` or backup-file disclosure. `accounts.key`, the environment root key, the full data directory, or compromise of the Server OS user remains sufficient to decrypt credentials; this is not a hardware-backed secret boundary.",
    "- S1/name/URL readers and `/api/provider-presets`, `/api/provider-matrix`, and `/api/provider-type` compatibility endpoints remain intentionally available. They cannot be removed until two stable bridge releases and at least 14 observation days are recorded in `provider-compatibility-window.json`; the current removal gate is not satisfied.",
    "",
    "### Account credentials, fixed binding, and recovery",
    "",
    "- `/api/accounts/capabilities` is the account-control-plane truth. In addition to the backward-compatible `supportsRefresh`, `supportsQuota`, and `supportsRefreshPlan` fields, it publishes `managerKind`, `refreshCapability`, `quotaCapability`, `supportsCachedQuota`, `supportsLiveQuotaRefresh`, `credentialOwnership`, `inferenceBindingSupported`, `deprecatedForInference`, and `migrationTarget`. The Web UI fails closed when this matrix is unavailable or a Provider type is absent.",
    "- OAuth and import-only managed types (`claude_oauth`, `codex_oauth`, `grok_oauth`, `gemini_cli`, `github_copilot`, `deepseek_account`, `kiro_oauth`, `cursor_oauth`, `antigravity_oauth`, and `agy_oauth`) keep credentials in `accounts.json`. A managed Provider binds one compatible Account identity; the initial request and every retry stay on exactly that binding and cannot select from a pool, rotate to another Account, or escape to another Provider. Agy and Antigravity remain separate account/provider labels even though they share Google protocol primitives.",
    "- Static API Key/AWS types (`cursor_apikey`, `ollama_cloud`, `aws_bedrock`, `nvidia`, and `deepseek_api`) are Provider-owned credentials. Their legacy Account rows are metadata/quota compatibility only, report `inferenceBindingSupported=false`, and have `migrationTarget=provider`; the Provider editor must not offer those rows as inference bindings. Direct `claude`, `claude_auth`, `codex`, `gemini`, and `openrouter` credentials are likewise Provider-owned rather than Account-managed.",
    "- Recovery never changes identity. Where a managed OAuth driver has an implemented refresh path, the first eligible 401 before downstream commit may force-refresh the same bound Account once and replay once. A protocol-specific API-key exchange/re-resolution, when implemented, is constrained to the same Provider credential. A second 401, an ambiguous write/body failure, or any post-commit failure is terminal; none of these paths authorize cross-account or cross-Provider scheduling.",
    "",
    "### Gemini Code Assist v1internal",
    "",
    "- Gemini CLI, Antigravity, and Agy use the Google Code Assist `v1internal` generate-content envelope and resolve `projectId` only for the exact Account bound to the Provider. OAuth exchange and generic account import attempt best-effort project enrichment; the first generating proxy request or Provider network test performs synchronous discovery when the project is still absent. `countTokens` is parsed as a distinct action and uses the OAuth AI Studio `/v1beta/models/{model}:countTokens` endpoint without a Code Assist envelope or project discovery.",
    "- Project discovery is singleflight per Account, generation-safe, and durably persists the discovered project and tier. Quota refresh preserves partial project/tier updates even when a later quota step fails, records failure and relogin state atomically, and observes the same-account cooldown.",
    "- Antigravity and Agy requests containing Google Search use `requestType=web_search` and the audited `gemini-2.5-flash` fallback model while preserving function tools; ordinary requests use `requestType=agent`. Gemini CLI omits Antigravity-only identity fields, and Agy never borrows an Antigravity Account even when local ids collide.",
    "- Top-level and nested `response.error` payloads are Provider failures. Streaming responses unwrap every `response` envelope across arbitrary chunk boundaries; non-stream aggregation and streaming EOF/`[DONE]` both require terminal candidate `finishReason` evidence or explicit blocked prompt feedback. Gemini-to-Claude bridges emit a complete Anthropic message/content/tool lifecycle, use `stop_reason=tool_use` for function calls, and include `thoughtsTokenCount` in output usage.",
    "- An eligible discovery, `countTokens`, or forwarding 401 may force-refresh the same bound Account once and replay once. The initial request and every retry remain on the configured Provider's explicit Account binding; saturation fails with 429, a managed execution never crosses Provider boundaries, and generic failover excludes managed Provider candidates. No path selects an account pool, rotates to another Account, or retries through another Account.",
    "- Request envelopes, action routing, streaming validation, non-stream aggregation, and protocol bridges are fixture-verified. Real Google OAuth credentials and Code Assist project entitlements were not available, so live Code Assist acceptance remains unverified.",
    "",
    "### OAuth Share identity, binding, and recovery",
    "",
    "- Every non-`deleted` OAuth Share validates the strong subscription identity of each OAuth binding. Claude identity is the normalized account UUID. Codex identity is the verified OpenAI subject paired with the effective verified workspace ID. Email, display name, local account row ID, token shape, and unverified workspace metadata are never identity fallbacks. The same strong identity may intentionally back multiple independent Share URLs; identity validation protects binding integrity rather than imposing global URL uniqueness.",
    "- Ordinary Share upsert/import may update mutable Share fields but cannot change existing bindings. Dedicated add/remove endpoints use `configRevision` CAS; adding a second or third app happens only after explicit reuse confirmation, requires an unshared Provider with the same credential source, and preserves one shared URL, ACL, limits, expiry, description, subdomain, and price. Replacing a binding requires `status=paused` and the same app. Codex workspace changes use the paused/revision-CAS rule, reject accounts referenced by another independent Share URL, advance the Account plus every same-account Provider generation within the accepted Share, and append the actual binding's redacted identity transition to `bindingHistory`.",
    "- Managed OAuth Provider compilation and outbound authentication require the bound Account generation to match. For a legacy OAuth Provider that contains both an explicit account binding and a static secret, the managed Account is authoritative; an OAuth type attached to a static-credential Profile cannot back a Share. Account deletion and ordinary Provider writes cannot invalidate or silently replace a Share binding.",
    "- The Codex workspace operation stages sealed `accounts.json`, `providers.json`, and `shares.json`, validates the complete reference graph, persists a digest-bearing commit marker, and rolls the committed transaction forward on startup and before later account/provider/share writes. Mutation lock order remains config → providers → accounts → usage → shares.",
    "- A Share invocation derives the requested app from the authenticated route and pins that app's stored binding; an unbound app fails closed. It never enters account-pool selection, rotation, random choice, capacity fallback, cross-account failover, or cross-Provider failover. Same-account token refresh, same-Provider protocol/transport retries, and Codex WebSocket-to-HTTP fallback using the original execution identity are allowed.",
    "",
    "### OAuth login and cross-protocol bridge contract",
    "",
    "- `/api/accounts/capabilities` publishes ordered `loginFlows` entries (`browser_oauth`, `device_code`, `cli_manual_callback`) with callback/poll/cancel support for each account type. `supportsStartLogin` and `supportsCallback` remain backward-compatible fields derived from that list; they are not a second capability source. Claude OAuth exposes browser plus CLI-manual callback, while Codex OAuth additionally exposes device code. A declared flow means the Server control path exists; it does not prove that production credentials, upstream entitlements, callback routing, or real inference have passed acceptance.",
    "- Shared non-stream transforms force function parameter roots to JSON objects without discarding other schema keywords, move image/document/audio tool-result media into native target-protocol parts, remove reasoning-only history where required, preserve multi-fragment summaries, and attach or backfill reasoning around tool calls. Opaque OpenAI reasoning and signed Anthropic thinking use distinct HMAC-SHA256 envelopes derived with HKDF from the accounts root key; malformed, oversized, cross-kind, or tampered envelopes fail closed instead of replaying unauthenticated metadata.",
    "- Responses↔Anthropic, Chat↔Anthropic, and Responses↔Chat streaming use per-request lifecycle state. Parallel tool identity/order, dense downstream tool indexes, packed or fragmented arguments, signed thinking replay, terminal completion, and EOF failure are covered across arbitrary chunk boundaries.",
    "- The Responses semantic guard is shared by JSON documents, HTTP/SSE streams, WebSocket frames, and WS→HTTP fallback. Lifecycle events do not commit downstream or satisfy first-token timing; the configured first-event timeout is one absolute deadline until the first business or terminal event. Client validation failures are returned without Provider penalty, Provider-origin failures may fail over only before downstream commit, and `response.incomplete` is a valid partial terminal. `CC_SWITCH_PROXY_SEMANTIC_GUARD_ENABLED=0` is the incident rollback for ordinary Responses; Responses image transports retain minimal lifecycle/terminal inspection because their heartbeat has already committed wire `200`. `/metrics` exposes `cc_switch_proxy_semantic_guard_total` plus authenticated reasoning outcomes.",
    "- Executable bridge cases are indexed by `assets/contract/proxy-bridge-protocol.json` and backed by `tests/fixtures/proxy_bridge`. Anthropic request normalization includes required `max_tokens`, system/developer precedence, supported request controls, complete adjacent tool turns, orphan cleanup, and fail-closed function arguments. These local contracts do not satisfy real Claude/OpenAI OAuth, ChatGPT upstream, Router, or Market acceptance gates.",
    "",
    "### `deepseek_account` (DeepSeek import-only account)",
    "",
    "- DeepSeek account password login remains excluded. The Server accepts only an access-token import with an optional account label; passwords are neither accepted nor stored. Add/list/remove/default operations use the real `AccountStore` rather than compatibility success stubs.",
    "- The capability is `managerKind=import_only`, `credentialOwnership=managed_account`, `refreshCapability=unavailable`, and `quotaCapability=cached_only`. Imported quota snapshots may be displayed, but there is no browser login, password exchange, native token refresh, or live quota refresh claim.",
    "- `deepseek_account` is distinct from `deepseek_api`: the latter is a Provider-owned static API Key profile and must not bind a metadata-only Account row.",
    "- Claude-to-DeepSeek account request/stream fixtures and PoW/request construction are local protocol evidence only. The Claude adapter remains planned and Codex/Gemini combinations remain skeleton/fallback until real DeepSeek token, non-stream, stream, tool, usage, expiry, and error evidence is recorded.",
    "",
    "### `aws_bedrock` (AWS Bedrock)",
    "",
    "- Bedrock AKSK/session-token and bearer API Key credentials are stored on the Provider, not selected from Account rows. There is no Bedrock OAuth or account-pool path.",
    "- Local contracts cover Converse body construction, region/model endpoint resolution, SigV4 canonical request/signature parts, session-token handling, redaction, and unique final authentication/content headers. These fixtures prove request construction only.",
    "- Runtime adapter capability remains `planned`; operational acceptance is `live_pending`. Registry visibility or an Experimental profile does not upgrade it to native, and Codex/Gemini Bedrock combinations remain planned.",
    "- Do not mark Bedrock live until a real AWS account independently passes non-stream, stream, tool use/result, image where supported, model/region errors, throttling, temporary credentials, and response-usage checks. No real AWS input was available for the current local validation.",
    "",
    "### `kiro_oauth` (Kiro OAuth)",
    "",
    "Server-native Kiro pass from external protocol evidence plus tool-call hardening (2026-07-13):",
    "",
    "- OAuth/account storage: Builder ID and IdC device flow share AWS SSO OIDC registration, `issuerUrl` is persisted for IdC re-registration, and Google/GitHub Social login uses Kiro's server-safe device authorization/poll endpoints. Native refresh is selected dynamically by `authMethod` for Builder ID/IdC/Social/External IdP; OIDC refresh 401 can re-register the client and retry once.",
    "- Imports: Kiro `credentials.json` can be pasted or read from the server host, and `ksk_` API keys are validated through `ListAvailableProfiles` before import. The account store recursively encrypts token/API-key/client-secret fields, including nested refresh responses.",
    "- Proxy: Claude-only Kiro forwarding builds CodeWhisperer IDE requests by default and can use the CLI endpoint when account metadata sets `endpoint=cli`; requests add API_KEY/EXTERNAL_IDP `tokentype` when needed, default `profileArn` by auth method, and fall back to profileArn-derived region. Base URL overrides retain the endpoint-specific IDE or CLI path, and final request assembly emits one protocol-owned Content-Type. EventStream parsing now validates prelude/message CRC and inline `<thinking>` content is split into Claude reasoning blocks.",
    "- Tool-call hardening: top-level tool input schemas are forced to objects and unsupported combinators are stripped with object-field recovery. Non-stream tool JSON is buffered until `stop=true`; invalid or incomplete JSON returns a stable non-retryable 502 code. `TOOL_SCHEMA_INVALID` and `TOOL_USE_RESULT_MISMATCH` bypass retry and Provider outcome accounting, and `ksk_` values are masked before Kiro errors enter logs.",
    "- Quota: `getUsageLimits` is available through the normal quota refresh path and refresh updates can backfill `kiroUsageLimits`.",
    "- Real Kiro upstream validation remains an external gate: do not mark Kiro native acceptance complete until a real Kiro account has exercised Claude non-stream, stream, usage refresh, refresh-token rollover, and rate-limit handling.",
    "",
    "### `claude_oauth` (Claude Official)",
    "",
    "Claude OAuth protocol evidence review through 2026-08-03, implemented independently in Server:",
    "",
    "- Proxy hot path: legacy-compatible and typed Claude OAuth Providers share one prepared-request contract for network tests and real forwarding: managed-account refresh, `?beta=true`, request-shape-driven `anthropic-beta` assembly (`claude-code-20250219`, `oauth-2025-04-20`, thinking/tools/computer/context/effort/1h-cache/explicit-1m only when allowed), Claude CLI headers, per-account stable stainless OS/arch profile, session metadata, billing/identity injection, thinking sampling normalization, preserve-order JSON, and one final `cch=` signature over the cleaned body. Known Claude Code tools use canonical wire casing across declarations, `tool_choice`, and history, while streaming/non-streaming responses restore the request's declared casing; ambiguous case-insensitive declarations fail closed and custom names remain unchanged. Repeated client beta headers are merged through a fail-closed allowlist, unknown values are dropped without logging their raw token, repeated case-insensitive `[1m]` suffixes are removed before final signing, OAuth omits browser-only headers, and account extra headers cannot override the signed contract.",
    "- Retry and semantic hardening: direct Claude requests resolve only the route-scoped Claude Surface under `/r/:routeKey`; request headers cannot override that binding, and Share requests retain their immutable Provider/account binding. Messages may replay once only for a connect-stage failure, once after a same-account forced refresh on 401, or through the bounded OAuth signature/thinking/web-search compatibility stages. Ambiguous send/timeout/body-read failures, 429/529, SSE error frames, and post-commit failures are never transparently replayed. `count_tokens` may retry bounded transport failures and one same-account 401 refresh because it is non-generating, but does not switch Provider/account. Native Anthropic JSON and SSE responses are incrementally validated for message/count shape, event ordering, terminal state, truncation, and bounded event/prelude size; downstream cancellation aborts the upstream body without penalizing Provider health.",
    "- Routes/usage/transform semantics: `/v1/messages/count_tokens` and `/claude/v1/messages/count_tokens` are available only through native `claude`, `claude_auth`, or `claude_oauth` providers; generation fields are removed, OAuth adds the token-counting beta and re-signs the final body, and the result is not recorded as generation usage. Normal generation usage remains four non-overlapping buckets. Cross-protocol SSE now buffers complete events across arbitrary chunks and keeps per-request Responses/Chat→Anthropic text/tool lifecycle, including parallel tools and packed argument done events.",
    "- Operations hardening: the quota refresh loop first warm-refreshes due native OAuth tokens and isolates accounts after repeated `invalid_grant` failures. Route-scoped Claude routing acquires the in-flight guard for the Bundle's single bound account (default limit 8, provider/env configurable); saturation returns 429 instead of selecting another account. Share routing likewise uses only its bound account. Rotated credentials are committed with atomic persistence and an in-memory degraded fallback plus generation-safe background retry; `/ready` and `cc_switch_credential_persistence_degraded` expose that state. Non-streaming version-gate responses are rewritten into admin-facing guidance to bump `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA`. Account identity generations follow provider type plus the strongest stable principal rather than scopes, auth shape, email casing, or ordinary profile enrichment. Downstream responses use an audited allowlist for `x-request-id`, `retry-after`, `x-should-retry`, and Anthropic rate-limit/priority/fast headers; cookies, server identity, and unreviewed headers are not copied.",
    "- OAuth web-paste/profile: `code#state` parsing, platform token endpoint first, platform User-Agent (`axios/1.13.6`). OAuth exchange performs a non-blocking `/api/claude_cli/bootstrap` lookup; quota refresh runs usage, profile, and bootstrap in parallel. A shared domain resolver evaluates usage `tier` / `plan` / `subscription_type`, bootstrap and profile rate-limit tiers, organization type, then compatible cached evidence. It publishes canonical `claude_max_5x` / `Claude Max 5x` and `claude_max_20x` / `Claude Max 20x` values to account state, quota subscription metadata, Auth Center, and account selectors. Generic Max remains `Claude Max` when no multiplier exists; incompatible live evidence keeps the highest-authority result and emits `claude_plan_conflict`, while a compatible cached multiplier is explicitly marked stale. Profile `billing_type` remains independent and is stored as `profile.billingSource` (`apple_subscription`, `stripe_subscription`, or a preserved unknown value) without deriving plan or expiry from it. Local protocol evidence contains an explicit 20x fixture; 5x remains live-unverified until the real-account gate in `real-acceptance-runbook.md` passes.",
    "- Beta/session hardening: Claude OAuth accepts client/body beta values only from protocol-owned or audited compatibility sets, removes internal beta fields from serialized JSON, and exports bounded decision metrics. OAuth login sessions can be cancelled atomically before exchange, cancellation is idempotent and terminal, completed sessions retain the imported account id for idempotent multi-tab completion, and unknown states remain rejected. Cancellation is rejected after token exchange starts.",
    "- Local callback uses `/api/accounts/login/callback`; Claude CLI callback route `/web-api/oauth/claude-cli/callback` is also registered, while a dedicated `127.0.0.1:54547` listener remains a deployment/productization choice.",
    "- Evidence-gated exclusions: wire header casing/order and TLS/JA3 impersonation are deferred until captures show they are required; tool cloaking is not enabled without an observed OAuth tool-name block. The 54547 listener and MITM/DNS interception are not part of the headless server requirement. Skill, MCP, Tauri, session-manager, and Claude Desktop profile mutation remain outside the server product boundary.",
    "",
    "### `codex_oauth` (OpenAI OAuth)",
    "",
    "Codex/OpenAI OAuth protocol evidence review through 2026-07-30, implemented independently in Server. External references are protocol evidence only, not Share account-pool architecture templates:",
    "",
    "- OAuth/account storage: Device OAuth and official CLI PKCE OAuth share the server login state machine. For the configured remote HTTPS Client URL, CLI OAuth preserves `http://localhost:1455/auth/callback`; after the browser's local redirect fails, the administrator submits the complete callback URL to the originating, principal-bound login session. The Server requires a signed Router ingress and same-origin Client URL request, then validates the exact callback origin/path, state and expiry before exchange. Every supported device flow binds start/poll/cancel to the authenticated administrator principal for the device-code lifetime; Codex polling is serialized, cancellable and idempotent. Refresh singleflight/backoff is scoped by account record and refresh token, duplicate refresh tokens are rejected, and `refresh_token_reused` immediately isolates the account. Token fields are encrypted in `accounts.json`, while control-plane responses expose only credential-presence booleans and sanitized runtime state; no plaintext account credential export endpoint is exposed.",
    "- OpenAI trust boundary: both ID and access JWTs use cached OpenAI JWKS with RS256, issuer, audience, expiry/nbf and `kid` rotation checks. One canonical extractor reads the literal `https://api.openai.com/auth` object (plus explicit legacy shapes), keeps user subject separate from `chatgpt_account_id`, continues from an empty ID-token identity to the verified access token, rejects conflicts, and requires both a non-empty subject and workspace. New local account record IDs are a stable SHA-256-derived subject ID; workspace remains only the upstream `chatgpt-account-id` identity. Existing records with the same verified subject are reused atomically, and refresh fails closed if a previously verified account returns a different subject. Workspace selection and the outbound header consume only verified claims or authenticated discovery provenance. The executable cases live in `assets/contract/openai-oauth-protocol.json`.",
    "- Endpoint and binding policy: managed Codex OAuth authorization, token, quota and inference endpoints are fixed to the audited official origins; provider/user endpoint overrides cannot receive OAuth credentials. Every managed OAuth Provider must bind a concrete compatible account. The headless server does not live-read or write the host user's `~/.codex/auth.json`.",
    "- Proxy headers/body: managed account requests finalize a paired official Codex identity (`originator`, configurable `version` defaulting to `0.144.1`, and User-Agent), inject the validated `chatgpt-account-id`, session/window headers, `reasoning.encrypted_content`, `prompt_cache_key`, and versioned instructions. Final outbound requests always set `store=false`; non-compact Responses always set upstream `stream=true`, including Claude/Gemini translations, while a downstream non-stream request is incrementally aggregated back into one Responses JSON document. String references and object IDs carrying server-only `rs_`, `fc_`, `resp_`, or `msg_` prefixes are removed, as are `item_reference` entries. Reasoning remains client-selected: nested `reasoning.effort` takes priority over promoted top-level `reasoning_effort`; explicit Claude `output_config.effort`/`thinking.effort` and Gemini `generationConfig.thinkingConfig.thinkingLevel` survive translation and are recorded as requested values; `low`, `medium`, `high`, `xhigh`, and `max` pass through, while `ultra` alone normalizes to `max`. Image generation/edit bridges preserve an explicit client effort instead of replacing it with their `medium` default. FAST is server-authoritative: inbound `service_tier` and `serviceTier` are always removed; a disabled Provider cannot be enabled by the client, while an enabled Provider forces `service_tier=priority` only when the resolved manifest/built-in model capability explicitly supports priority. Unsupported and unknown capabilities fail closed without priority. Cursor/Droid/Chat-compatible fields proven unsupported by Codex OAuth are removed without imposing a broader final allowlist. Native HTTP/SSE, Chat, Compact, Overflow, Images, Alpha Search, WebSocket, WS→HTTP fallback, Provider network tests, and Claude/Gemini requests translated through `oauth.openai_codex` share the same final policy. The built-in capability snapshot is aligned with the audited official model manifest, including explicit non-priority models.",
    "- Protocol/usage: Responses Lite `additional_tools`, custom/freeform history and response restoration, namespace flattening, `tool_search` downgrade/collision rejection, custom-tool stream completion, and strict wire zero fields are covered. OpenAI/Anthropic cache usage is normalized into fresh/read/write/output buckets, including nested `cache_write_tokens` and explicit zero values. Usage schema v4 records requested/effective reasoning effort, client/effective service tier, and the bounded server decision independently from model identity. Stream logs transition from `pending` to `observed`, `missing`, `parse_error`, or `interrupted`; forced-SSE aggregation for a downstream non-stream request creates the same terminal states on success, upstream failure, parse error, missing terminal, timeout, or interruption and always completes Share/Provider outcome accounting. `usageRevision` is monotonic, explicit observed zero remains distinct from unknown usage, and Router synchronization is revision-safe. Each WebSocket `response.create` owns one terminal Usage log across success, failure, cancellation, protocol error, and WS→HTTP fallback; later frames inherit the last request model unless `session.update.session.model` replaces it, so policy evaluation, replay, and observability stay aligned when a client omits the model.",
    "- Streaming/WS/images: Responses POST SSE keeps protocol conversion; Responses GET upgrades through WebSocket with a per-provider incident rollback toggle. Codex WS connections use a bounded pool keyed by process, Provider/runtime, session, upstream URL and credential/workspace headers, with capacity, idle TTL and max-age eviction. Connect/5xx handshake/stale-cache failures and send failures before `response.create` is accepted by the socket may replay through HTTP/SSE; after a successful send, read/close/1009/first-event-timeout failures terminate the lifecycle without transparent replay. The configured stream first-event timeout (default 120 seconds) covers request send, response headers, and that first valid event without being extended by SSE comments or partial bytes. After the first event, the idle timeout (default 300 seconds) only terminates the stream. Handshake 4xx and committed responses never trigger transport replay. HTTP fallback keeps the same execution/account/workspace/concurrency lease, supports flat and nested request frames, bounds one SSE event to 128 MiB and rematerializes auth after one same-account 401 refresh. SSE and WS `response.completed` events with empty output are rebuilt from prior `output_item.done`; Windows/Unix reset classification and big-frame `message_too_big` mapping are covered. Remote input images on ordinary Responses/WS/Cursor paths allow only HTTP(S) or validated data URIs, revalidate every redirect and DNS answer, pin the validated address, block private/reserved/transition IPv4/IPv6 ranges, cap 16 images and 1 MiB each with bounded concurrency/time, and require a supported MIME/signature. Dedicated Codex OAuth `/v1/images/generations` and `/v1/images/edits` bridge to the same-account Responses image tool, accept at most 16 inputs with 20 MiB per-image and 32 MiB aggregate limits, validate multipart/data-URI/remote MIME signatures and request parameters, and deliberately reject `n>1`. Explicit Responses image-tool requests, dedicated Images, and successful Grok image responses commit an SSE comment or legal JSON whitespace before long generation work and then emit 15-second heartbeats; these transport bytes do not extend upstream first-event/request deadlines. Once committed, wire HTTP status remains 200 and no transparent Provider retry is possible, so in-band errors and terminal usage status are authoritative. Missing terminal events, upstream failure, timeout, cancellation, and output bounds update terminal usage without recording Provider success; decoded image count/bytes/format/dimensions have dedicated logs and metrics. `response_format=url` uses a 256-bit, one-hour, no-store same-origin capability URL from a bounded durable file store. The default store survives restart; replicas sharing a lock-capable `CC_SWITCH_IMAGE_STORE_DIR` can serve the same URL, while independent stores require sticky routing. Cloudflare workers must pass `Response.body` through without buffering, set `CC_SWITCH_IMAGE_PUBLIC_BASE_URL` when origin Host is rewritten, and allow anonymous GET/HEAD for capability URLs.",
    "- Quota/subscription evidence: `/wham/usage.plan_type` is authoritative for the displayed plan. `/accounts/check` rejects expired or inactive candidates and uses exact matching for a verified workspace; `/subscriptions` is queried only for that verified workspace. Conflicting plans, untrusted workspace expiry, and past expiry contradicted by an available paid usage response are discarded, while sanitized resolution evidence is persisted for diagnostics. A discarded expiry is absent from both the auth summary and Share descriptor instead of being reported as expired. Explicit `code_review`/`codex_review`/`review` windows are exposed as separate `review_session`, `review_weekly`, or `review_monthly` tiers; malformed or empty candidates do not hide a later valid candidate, reset timestamps accept seconds or milliseconds, and review utilization does not overwrite the account's ordinary quota percentage.",
    "- Account-center and retry boundary: zero Codex OAuth accounts report `unconfigured`; one account is selected automatically; multiple accounts report `needs_selection` for account-center operations until the administrator chooses one. Selection persists only the account-center preference and never rebinds a Provider Bundle or Share. Direct traffic uses only the Route Key Surface and its bound account. HTTP, SSE, Images, models, alpha search, WS, and WS→HTTP fallback never enter candidate-account selection or cross-Provider/account failover. Codex 429 bodies parse `error.resets_in_seconds` and `error.resets_at`; generic managed-account handling honors bounded `Retry-After` but writes cooldown only to the bound account.",
    "- Overflow recovery: `CC_SWITCH_CODEX_OVERFLOW_AUTO_COMPACT=1` opt-in detects HTTP 400 and pre-commit `response.failed` context overflow, summarizes bounded older input with the same Provider/account, preserves recent context and tool pairing, then retries the original request once. Summary failure degrades to an omission marker, committed output is never replayed, and summary usage is recorded separately as `codex_overflow_compact_summary`. The feature is disabled by default and never calls the top-level Router recursively.",
    "- Client gate: inbound requests reject generic tool signatures while the final outbound header pass pairs official originator/User-Agent families and raises obsolete versions before every HTTP, WebSocket, and image request.",
    "- TLS fingerprint: no Chrome/TLS impersonation is implemented in server; current stance is rustls direct TLS plus header/client gating. Real ChatGPT upstream smoke should revisit this only if upstream starts rejecting rustls traffic.",
    "",
    "### `cursor_oauth` / `cursor_apikey` (Cursor AgentService)",
    "",
    "Cursor OAuth/API key protocol evidence review (2026-07-09), implemented independently in Server:",
    "",
    "- OAuth/account storage: DeepControl PKCE + poll remains the browser login path; server now also imports Cursor IDE `state.vscdb` from the cc-switch-server host and falls back to cursor-agent `auth.json` across Linux/macOS/Windows (`CURSOR_AGENT_AUTH_PATH` can override). Imported IDE tokens preserve `cursorServiceMachineId`; agent auth imports are accepted without machine id. `CURSOR_STATE_DB_PATH` can override the IDE DB path; vscdb reads use an immutable SQLite URI to avoid live Cursor WAL locks; OAuth, local import, and profile enrichment derive account ids from the same WorkOS subject hash when available. Account token fields are covered by the shared encrypted `accounts.json` store.",
    "- Profile enrichment: Cursor `/api/auth/me` uses the dashboard WorkOS session cookie shape (`WorkosCursorSessionToken=<workos_user_id>::<access_token>`) derived from the access-token JWT, not the generic `Authorization: Bearer` profile request. Token exchange/refresh, poll, and profile requests now share the Cursor browser-login User-Agent. Enrichment failure is non-fatal so access-token-only imports can still be used; when profile includes `sub`/`user_id`/`id`, it is used as the stable account id seed if tokens lack a subject.",
    "- Proxy transport: Claude/Codex/Gemini Cursor providers use the native HTTP/2 Connect-RPC AgentService driver by default, with provider/env settings able to disable it during incident triage. The driver covers AgentService protobuf frames, cursor-agent CLI headers, KV/session handling, built-in tool rejection, declared tools, images, and Anthropic/OpenAI Chat/OpenAI Responses/Gemini response formatting. AgentService headers include W3C `traceparent`/`backend-traceparent`; timezone comes from `TZ`; client version is detected from local Cursor state with a 60-minute cache and falls back to `cli-2026.01.09-231024f`.",
    "- Rate-limit hardening: AgentService 429 responses write `rateLimitedUntil` only to the Cursor Provider's explicit bound Account from `Retry-After` or Cursor JSON reset hints. Later requests remain on that binding and observe its cooldown instead of selecting another Account or Provider. Non-2xx AgentService responses read up to 8KB of JSON error detail (`error`, `message`, `code`, `details[0].message`) so clients see actionable diagnostics instead of status-only 502s.",
    "- Image boundary: Cursor's Anthropic/OpenAI Chat/OpenAI Responses/Gemini extractors use the shared case-insensitive HTTP(S)/data URI classifier. Remote loads use the shared DNS/redirect/IP/MIME/signature/count/concurrency/time limits, while native base64 branches reject payloads above the 1 MiB decoded bound before allocating their decoded buffer.",
    "- Real Cursor upstream validation remains an external gate: do not mark live Cursor OAuth/API key proxy acceptance complete until a real Cursor account has exercised streaming, tool call/result continuation, images, and rate-limit/cooldown behavior.",
    "",
    "### `grok_oauth` (Grok/xAI OAuth)",
    "",
    "Server-owned capability based on protocol evidence reviewed through 2026-07-27; it is not part of the external Provider baseline:",
    "",
    "- OAuth/account storage: xAI public client id, PKCE, `plan=generic`, `referrer=cc-switch-server`, workspace read/write scopes, browser nonce validation, serialized device polling, and strict RS256 OIDC/JWKS verification. Device start/poll advertise the shared CLI version plus `x-grok-client-surface: ui`; production authorize/token/discovery/JWKS endpoints are fixed to audited `auth.x.ai` HTTPS URLs, while loopback injection is test-only. Native refresh accepts an omitted replacement ID token only for an account with an existing verified subject, verifies any new ID token, and rejects subject changes. Explicit `~/.grok/auth.json` import also requires a signed ID token.",
    "- Proxy headers/body: OpenAI Responses upstream contract, `Authorization: Bearer`, `x-grok-conv-id`, Grok CLI identity defaulting to `0.2.111`, authoritative single-model routing with editable `grok-4.5` default, Responses field cleanup, reasoning effort/model/tool guards, and `encrypted_content` shape validation. `x-grok-turn-idx` is forwarded only from a valid downstream decimal u64; the server never fabricates or increments it, and the same optional value survives same-account 401 replay and WS→HTTP fallback.",
    "- Single-account retry boundary: every Grok Provider binds one concrete OAuth account. Initial authentication resolution, HTTP/JSON, SSE, media, WebSocket handshake, and WS→HTTP fallback use that explicit binding and never rotate accounts or enter generic cross-Provider failover; same-account 401 handling may force-refresh only that account once. Untargeted `/v1/models` requests do not select an arbitrary Grok Provider. Credential persistence degradation returns 503 before Grok data-plane traffic.",
    "- Media/WS/models: Grok images/videos routes forward to `api.x.ai/v1`; image edits translate common OpenAI multipart uploads to xAI JSON data URLs; Responses GET bridges to `wss://api.x.ai/v1/responses`. All local direct inference routes, including models and WebSocket handshakes, require the dedicated inference token and reject browser Origin headers unless router ingress was verified. Media and WebSocket capabilities default fail-closed and successful evidence is persisted per account. Model discovery uses a bounded ETag/TTL cache, last-known-good fallback, and exposes `source`, `stale`, and `fetchedAtMs`. Loopback WS/model endpoint injection exists only in test builds.",
    "- Rate limits/account cooldown: structured 429/reset hints and 401/403/5xx outcomes update only the bound account's cooldown/entitlement state; they never authorize selecting a different Grok account.",
    "- Quota/subscription expiry: weekly and monthly billing responses remain quota evidence only; `currentPeriod.end` and `billingPeriodEnd` are never treated as the payment/subscription expiry. An explicit expiry on an active subscription remains authoritative when available. Otherwise each Grok account can store a manual next-payment expiry, which survives OAuth refresh and is synchronized to provider and Share metadata without affecting credential validity or proxy scheduling.",
    "- Real xAI acceptance remains an external gate: local mock coverage does not claim live OAuth, non-stream/SSE, forced refresh, media, WebSocket, quota, or model-catalog success.",
    "",
  ];
}

function assertCoverage(coverage) {
  const missingTypes = coverage.providerTypes
    .filter((item) => item.required && !item.presentInSource)
    .map((item) => item.id);
  if (missingTypes.length > 0) {
    throw new Error(
      `Missing provider types in source: ${missingTypes.join(", ")}`,
    );
  }
  if (
    coverage.providerTypes.length !== serverLegacyInventory.providerTypes.length
  ) {
    throw new Error(
      "Provider coverage does not match the Server ProviderType inventory",
    );
  }
  const coveredIds = new Set(coverage.providerTypes.map((item) => item.id));
  for (const providerType of serverLegacyInventory.providerTypes) {
    if (!coveredIds.has(providerType.id)) {
      throw new Error(`Server ProviderType is not covered: ${providerType.id}`);
    }
  }
  for (const key of ["claude", "codex", "gemini"]) {
    if (coverage.presets[key].length === 0) {
      throw new Error(`No ${key} presets extracted`);
    }
  }
  assertRequiredProviderProfileCoverage(providerRegistry);
}

function writeIfChanged(file, content) {
  const existing = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null;
  if (existing === content) return false;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content);
  return true;
}

const coverage = buildCoverage();
assertCoverage(coverage);
coverage.fixtures = {
  claude: coverage.presets.claude.map((preset) =>
    providerFixture("claude", preset),
  ),
  codex: coverage.presets.codex.map((preset) =>
    providerFixture("codex", preset),
  ),
  gemini: coverage.presets.gemini.map((preset) =>
    providerFixture("gemini", preset),
  ),
};

const jsonPath = path.join(repoRoot, "assets/contract/provider-coverage.json");
const mdPath = path.join(repoRoot, "docs/provider-coverage.md");
const json = `${JSON.stringify(coverage, null, 2)}\n`;
const markdown = toMarkdown(coverage);

if (checkMode) {
  const actualJson = fs.existsSync(jsonPath)
    ? fs.readFileSync(jsonPath, "utf8")
    : "";
  const actualMd = fs.existsSync(mdPath) ? fs.readFileSync(mdPath, "utf8") : "";
  if (actualJson !== json || actualMd !== markdown) {
    throw new Error(
      "provider coverage assets/docs are out of date; run scripts/audit/audit-provider-coverage.mjs",
    );
  }
  console.log("provider coverage assets/docs are up to date");
} else {
  const changed =
    writeIfChanged(jsonPath, json) | writeIfChanged(mdPath, markdown);
  console.log(
    changed
      ? "provider coverage assets/docs updated"
      : "provider coverage assets/docs unchanged",
  );
}
