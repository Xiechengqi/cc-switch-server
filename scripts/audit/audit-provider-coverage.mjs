#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

import {
  assertRequiredProviderCoverage,
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
    customRecipes: providerRegistry.customRecipes.map((recipe) => ({
      recipeId: recipe.recipeId,
      label: recipe.label,
      profileId: recipe.profileId,
      compatibilityProviderType: recipe.compatibilityProviderType,
      binding: recipe.binding,
      modelPolicy: recipe.modelPolicy,
    })),
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
  lines.push(
    "> **自动生成文件 · 请勿手工编辑。** 由 `scripts/audit/audit-provider-coverage.mjs` 从 `assets/contract/provider-coverage.json` 生成；手工改动会被 `--check` 判为不同步。",
  );
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
  lines.push(
    "| ProviderType | Apps | Required | Present in pinned upstream baseline |",
  );
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
  lines.push("## Server Custom HTTP recipes");
  lines.push("");
  lines.push(
    "| Name | ProviderType | Profile | Protocol | Auth | Model policy |",
  );
  lines.push("| --- | --- | --- | --- | --- | --- |");
  for (const recipe of coverage.customRecipes) {
    lines.push(
      `| ${recipe.label} | \`${recipe.compatibilityProviderType}\` | \`${recipe.profileId}\` | \`${recipe.binding.upstreamProtocol}\` | \`${recipe.binding.authScheme}\` | \`${recipe.modelPolicy}\` |`,
    );
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
    "- Non-native and Custom Profiles default to `modelMapping.mode=single` with the Registry-owned non-empty `defaultUpstreamModel`, but may be switched explicitly to `passthrough`. Single mode overrides catalogs, direct mappings, rules, role-model environment variables, Copilot preflight normalization, and vendor-specific Kiro/DeepSeek/Grok/Kimi transforms. Passthrough retains the requested model while still allowing protocol adapters to normalize audited aliases. Cursor is the bounded exception: only an explicit Cursor alias/prefix may override the current Cursor Provider's single-model wire model/mode; it cannot select or escape the Share-bound Provider.",
    "- Provider load performs only an in-memory compatibility normalization and never rewrites `providers.json`. An allowed explicit mode and actual model are preserved; missing non-native mappings use the Profile default, while typed official records are repaired to passthrough. Legacy model values are inferred from app configuration when possible, and legacy Grok providers without an explicit mapping default to `grok-4.5`. S1-to-S2 cutover is an explicit offline CLI action; unresolvable historical records block cutover rather than being guessed.",
    "- HTTP usage records preserve the requested model, record the final upstream model and source, and attribute token usage to the final model. External Claude, Codex, and Gemini traffic is accepted only through verified Router Share ingress. The Share binding selects the protocol Surface and its Provider Bundle's bound account; request headers cannot override either identity. Codex OAuth active-account selection is account-center-only and does not affect routing. No Share route enters cross-Provider or cross-account failover. Grok image/video routes are intentionally excluded from usage aggregation.",
    "- Router ingress v2 uses the `cc-switch-router-ingress-v2` HMAC domain and binds the method, full path/query, SHA-256 of the exact forwarded body, and a unique per-send request id. Server verifies the signed envelope before bounded body collection, then verifies request binding and a 16,384-entry replay cache. Ordinary requests are capped at 2 MiB, media at 32 MiB, and Codex Images envelopes at 48 MiB. v1 compatibility ends after the inclusive `2026-09-08T00:00:00Z` boundary.",
    "",
    "### Provider control plane and storage",
    "",
    "- Rust `ProfileSpec` is the product identity authority, `DriverSpec` owns protocol operations, and each committed Provider compiles one canonical `RuntimePlan` shared by forwarding, manual test, and model discovery. Custom Profiles derive compatibility type deterministically from their explicit upstream protocol and authentication scheme; Anthropic Messages with Bearer authentication is classified as `claude_auth`. Named Custom HTTP recipes remain convenience configuration, not separate Provider families.",
    "- Every Driver declares an `outboundIdentityPolicy`, and the compiled RuntimePlan applies it as the last header step after protocol authentication and managed-account overrides. Claude/Codex/Grok/Kimi and Google Code Assist OAuth use their official CLI identity families; Kiro, Cursor, Copilot, and DeepSeek account drivers use their protocol-specific identities; Antigravity/agy use one background-refreshed client version and matching platform metadata; ordinary HTTP/API-key drivers use `cc-switch-server/<version>`; Bedrock omits User-Agent; frozen legacy Profiles retain their existing contract.",
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
    "- The serialized proxy `AdapterCapability.supportsOAuthRefresh` field is a legacy adapter-transform flag and remains `false`; protocol adapters do not own credentials or perform refresh. Managed Account refresh truth comes only from `/api/accounts/capabilities` and the same-account execution/recovery path above, so the legacy flag must not be used to infer whether Copilot or another Account type can refresh.",
    "- OAuth and import-only managed types (`claude_oauth`, `codex_oauth`, `grok_oauth`, `kimi_code`, `qoder_cosy`, `gemini_cli`, `github_copilot`, `deepseek_account`, `kiro_oauth`, `cursor_oauth`, `antigravity_oauth`, and `agy_oauth`) keep credentials in `accounts.json`. A managed Provider binds one compatible Account identity; the initial request and every retry stay on exactly that binding and cannot select from a pool, rotate to another Account, or escape to another Provider. Agy and Antigravity remain separate account/provider labels even though they share Google protocol primitives.",
    "- Static API Key/AWS types (`cursor_apikey`, `ollama_cloud`, `aws_bedrock`, `nvidia`, and `deepseek_api`) are Provider-owned credentials. Their legacy Account rows are metadata/quota compatibility only, report `inferenceBindingSupported=false`, and have `migrationTarget=provider`; the Provider editor must not offer those rows as inference bindings. Direct `claude`, `claude_auth`, `codex`, `gemini`, and `openrouter` credentials are likewise Provider-owned rather than Account-managed.",
    "- Recovery never changes identity. Where a managed OAuth driver has an implemented refresh path, the first eligible 401 before downstream commit may force-refresh the same bound Account once and replay once. A protocol-specific API-key exchange/re-resolution, when implemented, is constrained to the same Provider credential. A second 401, an ambiguous write/body failure, or any post-commit failure is terminal; none of these paths authorize cross-account or cross-Provider scheduling.",
    "",
    "### Provider-owned API-key Coding Plans",
    "",
    "- The Registry contains 70 Profiles, including 20 typed `codingPlan` Profiles across Claude and Codex. Each contract fixes origin, upstream protocol, credential slot/auth scheme, route, reviewed model catalog, quota adapter, cache-token semantics, stream terminal, error envelope, and same-credential retry policy. These are Provider-owned credentials and never participate in Account pooling, rotation, quota selection, or cross-Provider fallback.",
    "- Alibaba Coding Plan is split into China and Global/Singapore Families. Claude uses `x-api-key` against `/apps/anthropic/v1/messages`; Codex uses Bearer against `/v1/chat/completions`. Both regions have fixed DashScope Coding origins and reviewed catalogs. No stable official quota endpoint was evidenced, so the quota adapter is explicitly `unavailable` rather than inferred from console cookies or a different Alibaba Token Plan.",
    "- Within the Provider-owned Zhipu Coding Plan, `glm-5.3` is advertised only by the China and Global Codex Profiles. The reviewed 9router live receipt covers the OpenAI-compatible Coding endpoint and `reasoning_content`; it does not establish an Anthropic Messages rail, so the two Claude Profiles deliberately stop at their separately evidenced catalogs. Qoder independently maps a same-named model under the separate Qoder entitlement and never supplies Zhipu credentials.",
    "- Registry and fixture verification establish only a local contract. Alibaba and the expanded GLM catalog remain `experimental` / `live_pending` in this Server until each region and Surface has a real inference receipt; checked-in fixtures never claim live success.",
    "",
    "### Ollama API Key account and usage projection",
    "",
    "- Protocol evidence is the official API-Key JSON shape observed on 2026-08-14 and the matching 9router implementation at `15223724c3e1ad898e84ef6e0cc1686cbafc8290`: `POST https://ollama.com/api/me` returns account/profile/plan data, while `GET https://ollama.com/api/usage` returns `session` and `weekly` utilization ratios, per-window model request counts, and optional activity cost/period data. TokenRouter/sub2api at `a63b6b6077738d7e2222f02ec050b70d3aeb3516` instead scrape `/settings` with an encrypted browser session; that Cookie/HTML path and its distributed account-group scheduler are not adopted.",
    "- Ollama remains a Provider-owned static credential. The authenticated `GET`/`POST /api/providers/:id/account-usage?app=...` and Web invoke equivalents materialize only the canonical Bundle credential source, never create an Ollama Account row, and never make account identity eligible for inference binding, pooling, rotation, quota-based selection, or cross-Provider fallback. Account and usage retrieval are display-only and cannot block inference.",
    "- The fixed-origin client issues the two official requests concurrently with a sensitive Bearer header, exact methods and paths, an explicit empty `/api/me` POST, a 15-second timeout, a 512 KiB response limit, and redirects disabled. It validates bounded typed fields while treating drift in optional account metadata as absent, preserves `0%`, rejects invalid ratios and unsafe values, distinguishes authentication/rate-limit/transient/invalid-response failures, and never returns upstream response bodies or the API key in public errors.",
    "- Cache and concurrency scope is `(credentialSourceKey, credentialGeneration)`. Claude/Codex Surfaces in one Ollama Bundle share one singleflight and one memory-only account/usage snapshot. Fresh data lives for five minutes; transient or rate-limited endpoint failures may preserve that endpoint's prior data for up to one hour, while authentication failure clears it. Partial success is represented per section. Provider deletion and credential rotation immediately prune old in-memory identity data, and a result fetched with an old generation cannot be committed after rotation.",
    "- A successful Provider commit that introduces a new Ollama credential generation schedules one non-transactional background refresh; preset, ordinary, Bundle, import, and explicit identity-change writes all pass through that state commit boundary. The Web card also supports an exact-Provider manual refresh and displays plan, one email-first account identifier, session/weekly utilization, model request counts, activity cost, partial/stale/error state, and a real `0%` value. Successful snapshots use the same timestamp-only metadata row as OAuth cards, while exceptional states retain an explicit status label. The HTTP GET response is `private, no-store`, and the browser query cache is discarded as soon as the Provider observer is removed.",
    "- A cached Ollama identity/usage snapshot is overlaid only after the static Share descriptor fingerprint is computed. The outbound Router projection carries email, plan, session/weekly utilization, activity cost, and observation time, and a completed refresh force-syncs every Share bound to the same credential generation. Router renders the unprefixed plan, localized session/weekly labels, one-decimal utilization when needed, a one-decimal activity cost, and the email-first identity. This remains memory-only and display-only on the Server: it does not create an Account, mutate the persisted Share runtime snapshot, or affect inference availability, health, quota blocking, or scheduling.",
    "- Local client, state, API, query, and rendering contracts cover methods/headers, body bounds, error privacy, partial/stale merge, Bundle singleflight, post-commit warmup, deletion/rotation fences, session authorization, response redaction, revision scope, and zero-value display. `cargo test ollama_cloud_live_account_usage_from_env -- --ignored` is an optional real-account smoke requiring `OLLAMA_API_KEY`; a missing environment input is not live evidence.",
    "",
    "### Gemini Code Assist v1internal",
    "",
    "- Gemini CLI, Antigravity, and Agy use the Google Code Assist `v1internal` generate-content envelope and resolve `projectId` only for the exact Account bound to the Provider. OAuth exchange and generic account import attempt best-effort project enrichment; the first generating proxy request or Provider network test performs synchronous discovery when the project is still absent. `countTokens` is parsed as a distinct action and uses the OAuth AI Studio `/v1beta/models/{model}:countTokens` endpoint without a Code Assist envelope or project discovery.",
    "- Project discovery is singleflight per Account, generation-safe, and durably persists the discovered project and tier. Quota refresh preserves partial project/tier updates even when a later quota step fails, records failure and relogin state atomically, and observes the same-account cooldown.",
    "- Account control-plane responses expose a non-secret `capabilityEvidence` projection for the Gemini Code Plan. `credential_flow`, `project_provisioning`, and `model_entitlement` are independent: configured token/key state describes only the credential rail; successful `loadCodeAssist` supplies project evidence; only a successful `retrieveUserQuota` response containing a usable model id supplies positive model-entitlement evidence. Every observed value records the originating `authIdentityGeneration` and an expiry; expired evidence is `stale`, and evidence from an older identity generation is `superseded`. Raw project identifiers remain internal and are redacted from public quota payloads.",
    "- `gemini_cli` is the current Code Assist OAuth rail. Direct `gemini` API Key credentials remain Provider-owned AI Studio API access and are explicitly `unsupported` as Code Plan entitlement. Google One, AI Studio OAuth, and Vertex Service Account variants observed in reference projects are not advertised by this Server until separate credential contracts, drivers, and acceptance evidence exist.",
    "- Antigravity and Agy additionally expose a distinct `antigravity_code_plan` projection without replacing the generic Gemini projection. Its `project_bootstrap`, `privacy`, `tier_entitlement`, Gemini/Claude quota-family, and model-capacity dimensions are current-generation and expiring. Project/tier evidence comes from `loadCodeAssist`; family and capacity evidence requires explicit model buckets from `retrieveUserQuota`, and an absent family remains `unknown`. A bounded `fetchUserInfo` probe observes privacy read-only: an empty `userSettings` object is positive evidence, any `telemetryEnabled` field is negative evidence, and missing fields or probe failures are unknown. The Server never invokes `setUserSettings` or stores project ids in capability evidence.",
    "- Antigravity and Agy requests containing Google Search use `requestType=web_search` and the audited `gemini-2.5-flash` fallback model while preserving function tools; ordinary requests use `requestType=agent`. Gemini CLI omits Antigravity-only identity fields, and Agy never borrows an Antigravity Account even when local ids collide.",
    "- Antigravity/Agy apply a final managed-header scrub after Account overrides, removing forwarding, browser `sec-*`, Stainless, internal, compression, and spoofed Google client fingerprints before writing one Server-controlled User-Agent/client-metadata tuple. Structured Google RPC recovery accepts only HTTP/status/reason pairs `429/RESOURCE_EXHAUSTED/RATE_LIMIT_EXCEEDED` and `503/UNAVAILABLE/MODEL_CAPACITY_EXHAUSTED`, plus a valid `ErrorInfo.metadata.model` and bounded `RetryInfo.retryDelay`. Delays up to two seconds may replay once on the exact same Provider/Account generation before downstream commit. Longer or repeated limits return the original response and cool only the structured model within the current Share/runtime namespace; they never cool the whole account or choose another identity.",
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
    "### `github_copilot` (GitHub Copilot)",
    "",
    "- A managed Copilot Provider binds one explicit `github_copilot` Account. github.com and GHES device flows retain the long-lived GitHub OAuth credential only in the Account control plane. Before forwarding or model discovery, the Server exchanges it through that domain's `copilot_internal/v2/token` endpoint and sends only the resulting short-lived Copilot token to the data plane.",
    "- Endpoint metadata from token and internal-user responses is fail-closed. A production origin must be HTTPS, credential-free, query/fragment-free, rooted at `/`, and either an audited public Copilot host or a host in the exact configured GHES domain family. Account-controlled response data cannot create an arbitrary outbound target.",
    "- Model discovery calls the validated Account endpoint with the short-lived token and the managed Copilot editor/plugin identity. Catalog caches include Account id, `authIdentityGeneration`, `tokenRefreshGeneration`, GitHub domain, and validated API origin. github.com may return the audited public static catalog only as stale compatibility evidence after discovery failure; GHES never borrows that public catalog.",
    "- Live quota refresh calls `copilot_internal/user` with the GitHub OAuth credential and parses paid `quota_snapshots.premium_interactions` plus free/limited monthly shapes, including `unlimited` and authoritative reset time. The public `github_copilot_code_plan` projection independently reports credential flow, token exchange, endpoint provenance, model catalog, and premium interactions; every observation expires and is fenced by `authIdentityGeneration`.",
    "- Claude Messages, Codex Responses/Chat, and Gemini generateContent use fixture-verified Native Chat bridges with non-streaming and streaming tool lifecycles. The Gemini bridge preserves function calls, usage, finish state, and a single stream terminal. The first eligible 401 may exchange and replay once on the exact same Account and identity generation; it cannot select another Account or Provider.",
    "- Local Copilot contracts are fixture-verified, including github.com/GHES exchange and endpoint rules, model discovery, quota, capability evidence, token-only forwarding, and same-account replay. Real github.com/GHES device flow, models, quota, and inference remain `live_pending` until external credentials exercise the documented acceptance paths.",
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
    "Server-native Kiro protocol implementation reviewed through 2026-08-13 against OmniRoute `918fba5e392ce8b137976349f035597196edc440` and 9router `15223724c3e1ad898e84ef6e0cc1686cbafc8290`. Local fixture verification does not imply live-account acceptance:",
    "",
    "- OAuth/account storage: Builder ID and IdC device flow share AWS SSO OIDC registration, `issuerUrl` is persisted for IdC re-registration, and Google/GitHub Social login uses Kiro's server-safe device authorization/poll endpoints. Native refresh is selected dynamically by `authMethod` for Builder ID/IdC/Social/External IdP; OIDC refresh 401 can re-register the client and retry once. Authentication uses only the validated `authRegion`; runtime calls never inherit an arbitrary auth endpoint or region.",
    "- Runtime identity and imports: Kiro `credentials.json` can be pasted or read from the server host, and `ksk_` API keys are validated through `ListAvailableModels` before import. Profile ARN parsing requires the `arn:aws:codewhisperer:{region}:{12-digit-account}:profile/{id}` shape. Builder ID and Social may use their audited shared profile ARN, API Key is explicitly profileless, and IdC/External IdP must have a real discovered organization ARN. Profile discovery is sequential and bounded to `us-east-1`, `eu-central-1`, plus one validated auth region. A profile ARN's region overrides `runtimeRegion` and legacy `apiRegion`; invalid region/ARN input fails before host construction. The account store recursively encrypts token/API-key/client-secret fields, including nested refresh responses.",
    "- Refresh migration: the historical shared enterprise fallback ARN is never valid for runtime. When an old account refreshes, the rotated token receipt is persisted first; the fake ARN is then cleared and the account enters `profile_resolution_required`. A successful bounded discovery writes a second receipt with the real organization ARN. Model, quota, and inference remain fail closed between those receipts. A real prior organization ARN remains identity-fenced and cannot drift during refresh.",
    "- Binding and recovery: each Kiro Provider Surface binds one explicit `kiro_oauth` Account. Initial resolution, model discovery, generation, and the only eligible 401 replay remain on that exact account and identity generation. A pre-commit 401 may force-refresh it once and replay once, for at most two inference requests. Account pools, rotation, grouping, cross-account union/failover, cross-Provider failover, and decoy-account access are absent by contract.",
    "- Routes and endpoints: Claude Messages plus local `count_tokens`, Codex Responses, and Codex Chat Completions are native adapters over one Anthropic canonical bridge; non-stream and stream envelopes are fixture-verified for all three generation routes. Gemini Kiro remains unadvertised and unsupported. Production IDE and CLI inference URLs are fixed to `https://q.{authoritative-runtime-region}.amazonaws.com`; Provider settings cannot override them. IDE and CLI retain distinct paths, content types, target headers, identities, and body rules.",
    "- Wire semantics: the bounded AWS EventStream decoder validates total/header lengths, prelude CRC, message CRC, complete headers/payloads, known message types, and clean EOF. The first-complete-frame deadline is absolute from request send; local `message_start` and partial bytes neither satisfy nor extend it. Idle timeout resets only after a complete valid frame. Streaming and non-streaming success both require `endEvent`; frames after terminal, truncation, CRC corruption, oversized buffers, unknown message types, malformed typed events, error/exception frames, and missing terminal evidence fail closed with stable Kiro error codes. Timeout maps to 504, never triggers refresh/replay, and releases the account lease. Unknown event types still require valid JSON before being ignored. Claude, OpenAI Responses, and OpenAI Chat bridges have equivalent terminal fixtures.",
    "- Tool and image hardening: top-level tool input schemas are forced to objects and unsupported combinators are stripped with object-field recovery. Tool input is withheld until `stop=true` and complete JSON validation, including across multiple frames; pending tool JSON is capped at 16 MiB, and invalid, incomplete, or oversized JSON returns a stable non-retryable 502 code. Claude Code built-in tool names and inputs are bridged in both directions. Image inputs require string base64 `source.data` and use magic-byte MIME detection, bounded decoding/resizing/output budgets, newest-copy deduplication, and image extraction from tool results. `ksk_` values are masked before errors enter logs.",
    "- Discovery and caches: bound-account `ListAvailableModels` results replace rather than union with configured/static models, including an authoritative successful empty list. Identity and credential validation happens before cache lookup. Cache identity includes `account_id`, `auth_identity_generation`, `token_refresh_generation`, authoritative profile ARN (or explicit profileless API-key marker), and runtime region. Unresolved enterprise identity, absent credentials, and non-retryable upstream 4xx return an explicit empty catalog and cannot use stale/static models. Only a transient network/timeout/408/429/5xx or response failure after identity validation may use the same cache scope's bounded stale/static compatibility catalog. No model catalog is merged across accounts. Prompt-cache keys additionally namespace Provider, account, route, and session. `count_tokens` uses a local estimator, sends no inference request, and records no generation usage.",
    "- Quota: `getUsageLimits` is available through the normal quota refresh path and refresh updates can backfill `kiroUsageLimits`. All resource breakdowns are preserved independent of array order. Only exhausted `AGENTIC_REQUEST` with overage disabled blocks inference; `CODE_REVIEW` does not. Active trial/bonus contributions and reset evidence are retained, while an empty breakdown is connected-but-unavailable rather than a fake zero or unlimited quota.",
    "- Evidence: `assets/contract/kiro-wire-protocol.json` is consumed directly by Rust endpoint, image, decoder, runtime-model, and three-surface bridge tests. The Kiro-focused local suite currently passes 127 tests. Local contract status is `fixture_verified`; real Kiro upstream validation remains `live_pending` until a real Builder ID/IdC/Social/API Key account exercises Claude and Codex non-stream/stream/tools/images, cross-region model discovery, usage refresh, refresh-token rollover/profile migration, 401 replay, throttling, and stable error mapping.",
    "",
    "### `claude_oauth` (Claude Official)",
    "",
    "Claude OAuth protocol evidence review through 2026-08-26, implemented independently in Server:",
    "",
    "- Proxy hot path: legacy-compatible and typed Claude OAuth Providers share one prepared-request contract: managed-account refresh, `?beta=true`, ordered model/body-driven betas, CLI/Stainless identity, fail-closed native/helper classification, global cache-control constraints, preserve-order JSON, and one final CCH over the cleaned body. The 2.1.234 profile uses seed `0x4D659218E32A3268`, recursively blanks model strings, and excludes `max_tokens`, `fallbacks`, and `fallback_credit_token`; golden vectors verify the result. UA or billing alone cannot enter native minimal passthrough. Known tools retain canonical casing and response round-trip restoration. Optional custom/MCP aliases are session-scoped, default off, collision-safe, and cover declarations, forced choice, history, JSON and SSE while skipping server tools. Unknown betas are dropped without raw logging; obsolete fine-grained-streaming/computer-use betas are not synthesized.",
    "- Retry and semantic hardening: verified Router Share ingress resolves only the Share-bound Claude Surface and immutable Provider/account binding. Messages may replay once only for connect-stage failure, once after same-account 401 refresh, or through bounded body compatibility stages. 429/529 never replay. Claude 429 scope is explicit: unified 5h/7d rejection writes the bound account CAS cooldown; Fable `7d_oi`, ordinary and unknown 429 write only current Share+model cooldown; Fast credits refusal writes none. Reset parsing supports relative seconds, epoch seconds/milliseconds, RFC3339 and HTTP-date with a bounded horizon. `count_tokens` stays on the same identity. Native JSON/SSE validation and downstream cancellation semantics remain unchanged.",
    "- Routes/usage/transform semantics: `/v1/messages/count_tokens` and `/claude/v1/messages/count_tokens` use native upstream counting through `claude`, `claude_auth`, or `claude_oauth`; `kiro_oauth` serves the same routes with a local-only estimator and sends no inference request. Generation fields are removed from native count requests, OAuth adds the token-counting beta and re-signs the final body, and neither native nor local count results are recorded as generation usage. Normal generation usage remains four non-overlapping buckets. Cross-protocol SSE now buffers complete events across arbitrary chunks and keeps per-request Responses/Chat→Anthropic text/tool lifecycle, including parallel tools and packed argument done events.",
    "- Operations hardening: the quota refresh loop first warm-refreshes due native OAuth tokens and isolates accounts after repeated `invalid_grant` failures. A cancelled waiter cannot cancel the token-rotation owner; each owner has an independent 30-second deadline, panic and unknown post-send outcomes converge fail-closed, and graceful shutdown drains existing owners for up to 35 seconds. Share-scoped Claude routing refreshes before acquiring the Bundle's single-account inference lease, releases the lease before a 401 refresh, and reacquires it only for the same-account replay. Saturation returns 429 instead of selecting another account. Rotated credentials are committed with atomic persistence and an in-memory degraded fallback plus generation-safe background retry; `/ready` and `cc_switch_credential_persistence_degraded` expose that state. Non-streaming version-gate responses are rewritten into admin-facing guidance to bump `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA`. Account identity generations follow provider type plus the strongest stable principal rather than scopes, auth shape, email casing, or ordinary profile enrichment. Downstream responses use an audited allowlist for `x-request-id`, `retry-after`, `x-should-retry`, and Anthropic rate-limit/priority/fast headers; cookies, server identity, and unreviewed headers are not copied.",
    "- OAuth web-paste/profile: `code#state` parsing, platform token endpoint first, platform User-Agent (`axios/1.15.2`). OAuth exchange performs a non-blocking `/api/claude_cli/bootstrap` lookup; quota refresh runs usage, profile, bootstrap, and `/api/oauth/claude_cli/roles` in parallel with bounded body reads. A shared domain resolver evaluates usage `tier` / `plan` / `subscription_type`, bootstrap and profile rate-limit tiers, organization type, then compatible cached evidence. It publishes canonical `claude_max_5x` / `Claude Max 5x` and `claude_max_20x` / `Claude Max 20x` values to account state, quota subscription metadata, Auth Center, and account selectors. Generic Max remains `Claude Max` when no multiplier exists; incompatible live evidence keeps the highest-authority result and emits `claude_plan_conflict`, while a compatible cached multiplier is explicitly marked stale. Cached multiplier evidence retains its original `planObservedAt`, cannot be renewed by a stale quota query, expires after 24 hours, and then yields to live generic Max evidence. Profile `billing_type` remains independent and is stored as `profile.billingSource` (`apple_subscription`, `stripe_subscription`, or a preserved unknown value) without deriving plan or expiry from it. Local protocol evidence contains an explicit 20x fixture; 5x remains live-unverified until the real-account gate in `real-acceptance-runbook.md` passes.",
    "- Wire profile and discovery: `assets/contract/claude-oauth-wire-profile.json` records a sanitized installed-binary audit for Claude Code `2.1.234`, Stainless `0.112.1`, Node `v26.3.0`, and Axios `1.15.2`; it excludes credentials, identifiers, raw bodies and private build metadata. It versions endpoint identities, CCH/beta rules and the Fable 5, Opus 5, Opus 4.8, Sonnet 5, Opus/Sonnet 4.6 and Haiku 4.5 catalog. Discovery performs zero OAuth/upstream requests and remains fixture-verified, not real-account verified.",
    "- Cache/encoding/credentials: the final cache pass scans tools→system→messages, normalizes TTL order, keeps at most four high-value breakpoints, re-anchors recent content, and runs before CCH. Response governance supports gzip/x-gzip, deflate, br, zstd, repeated/comma encodings, four-layer reverse decoding and headerless gzip/zstd strong-magic detection with cumulative bounds. Claude credentials explicitly record `refreshable_oauth` or `access_only_setup_token`; setup tokens do not enter refresh, profile permission failures are fail-soft, internal identity uses a domain-separated digest when needed, and expiry requires re-import rather than looping refresh.",
    "- Beta/session hardening: Claude OAuth accepts client/body beta values only from protocol-owned or audited compatibility sets, removes internal beta fields from serialized JSON, and exports bounded decision metrics. OAuth login sessions can be cancelled atomically before exchange, cancellation is idempotent and terminal, completed sessions retain the imported account id for idempotent multi-tab completion, and unknown states remain rejected. Cancellation is rejected after token exchange starts.",
    "- Local callback uses `/api/accounts/login/callback`; Claude CLI callback route `/web-api/oauth/claude-cli/callback` is also registered, while a dedicated `127.0.0.1:54547` listener remains a deployment/productization choice.",
    "- Evidence-gated exclusions: wire header casing/order and TLS/JA3 impersonation remain deferred because no reproducible rejection proves they are needed. Dateline normalization and custom/MCP aliasing are independently disabled-by-default A/B gates and never apply to native/helper traffic. The 54547 listener and MITM/DNS interception are outside the headless server requirement. Skill, Tauri, session-manager, and Claude Desktop profile mutation remain outside the server product boundary.",
    "",
    "### `codex_oauth` (OpenAI OAuth)",
    "",
    "Codex/OpenAI OAuth protocol evidence review through 2026-08-26, implemented independently in Server. External references are protocol evidence only, not Share account-pool architecture templates:",
    "",
    "- OAuth/account storage: Device OAuth and official CLI PKCE OAuth share the server login state machine. For the configured remote HTTPS Client URL, CLI OAuth preserves `http://localhost:1455/auth/callback`; after the browser's local redirect fails, the administrator submits the complete callback URL to the originating, principal-bound login session. The Server requires a signed Router ingress and same-origin Client URL request, then validates the exact callback origin/path, state and expiry before exchange. Every supported device flow binds start/poll/cancel to the authenticated administrator principal for the device-code lifetime; Codex polling is serialized, cancellable and idempotent. Refresh singleflight/backoff is scoped by account record and refresh token, duplicate refresh tokens are rejected, and `refresh_token_reused` immediately isolates the account. Token fields are encrypted in `accounts.json`, while control-plane responses expose only credential-presence booleans and sanitized runtime state; no plaintext account credential export endpoint is exposed.",
    "- OpenAI trust boundary: both ID and access JWTs use cached OpenAI JWKS with RS256, issuer, audience, expiry/nbf and `kid` rotation checks. One canonical extractor reads the literal `https://api.openai.com/auth` object (plus explicit legacy shapes), keeps user subject separate from `chatgpt_account_id`, continues from an empty ID-token identity to the verified access token, rejects conflicts, and requires both a non-empty subject and workspace. New local account record IDs are a stable SHA-256-derived subject ID; workspace remains only the upstream `chatgpt-account-id` identity. Existing records with the same verified subject are reused atomically, and refresh fails closed if a previously verified account returns a different subject. Workspace selection and the outbound header consume only verified claims or authenticated discovery provenance. The executable cases live in `assets/contract/openai-oauth-protocol.json`.",
    "- Endpoint and binding policy: managed Codex OAuth authorization, token, quota and inference endpoints are fixed to the audited official origins; provider/user endpoint overrides cannot receive OAuth credentials. Every managed OAuth Provider must bind a concrete compatible account. The headless server does not live-read or write the host user's `~/.codex/auth.json`.",
    "- Proxy headers/body: managed account requests finalize a paired official Codex identity (`originator`, configurable `version` defaulting to `0.144.1`, and User-Agent), and inject the validated `chatgpt-account-id`, session/window headers, `reasoning.encrypted_content`, and `prompt_cache_key`. Existing `instructions` is preserved exactly and only a missing field defaults to `\"\"`. All official OAuth Responses transports set `store=false` and upstream `stream=true`; downstream non-stream requests aggregate the SSE terminal back into one JSON document. Public `/responses/compact` now uses ordinary upstream `/responses` with one final `compaction_trigger`; ordinary Responses carrying that trigger stay on `/responses`, while non-official providers retain their declared compact endpoint. The final HTTP sanitizer removes only a top-level `type=response.create`. Tool schemas recursively drop `type:null`, including Responses Lite `additional_tools`, while reserved `collaboration.*` tools pass through verbatim. Reasoning remains client-selected and only `ultra` normalizes to `max`. FAST remains server-authoritative and capability-gated. Native HTTP/SSE, Chat, compact, Images, Alpha Search, WebSocket, WS→HTTP fallback, Provider network tests, and Claude/Gemini translations share the audited final contract.",
    "- Lite, metadata, and routing: Responses Lite is decided after final model mapping from manifest/built-in `use_responses_lite`; explicit unsupported models lose both HTTP/WS signals and Lite body mutation, supported models retain it, and unknown models remain compatibility-pass-through. Existing turn/client metadata is scrubbed consistently across HTTP, same-account 401 replay, native WS, WS→HTTP fallback, and Images: workspace paths become account/generation/workspace/runtime-scoped stable placeholders, remote URLs are removed, and commit hashes become stable 40-hex placeholders without inventing absent client fields. Optional malformed/oversized turn metadata is dropped without echo. `x-codex-routing-hint` is server-owned and disabled by default; when enabled it is synthesized only from the final HTTP model and verified priority tier. Client/account overrides are rejected, and reusable WebSocket handshakes never carry a stale hint.",
    "- Text stream keepalive and previous-response recovery: ordinary Codex Responses SSE arms a downstream `: keepalive` only after a business/terminal event has committed; the default is 15 seconds, configurable from 5 to 60 seconds or disabled, and its clock never extends the independent upstream first-event/idle deadline. Previous-response tool context remains Share/principal/runtime/workspace scoped and bounded; cache statistics and short bounded tombstones cover expiry/eviction/rejection. A miss returns stable 409 `response_context_unavailable` only when the current request contains an unpaired tool output that actually requires the prior call; ordinary continuations and complete current call/output pairs do not fail.",
    "- Protocol/usage: Responses Lite `additional_tools`, custom/freeform history and response restoration, namespace flattening, `tool_search` downgrade/collision rejection, custom-tool stream completion, and strict wire zero fields are covered. OpenAI/Anthropic cache usage is normalized into fresh/read/write/output buckets, including nested `cache_write_tokens` and explicit zero values. Usage schema v4 records requested/effective reasoning effort, client/effective service tier, and the bounded server decision independently from model identity. Stream logs transition from `pending` to `observed`, `missing`, `parse_error`, or `interrupted`; forced-SSE aggregation for a downstream non-stream request creates the same terminal states on success, upstream failure, parse error, missing terminal, timeout, or interruption and always completes Share/Provider outcome accounting. `usageRevision` is monotonic, explicit observed zero remains distinct from unknown usage, and Router synchronization is revision-safe. Each WebSocket `response.create` owns one terminal Usage log across success, failure, cancellation, protocol error, and WS→HTTP fallback; later frames inherit the last request model unless `session.update.session.model` replaces it, so policy evaluation, replay, and observability stay aligned when a client omits the model.",
    "- Streaming/WS/images: Responses POST SSE keeps protocol conversion; Responses GET upgrades through WebSocket with a per-provider incident rollback toggle. Codex WS connections use a bounded pool keyed by process, Provider/runtime, session, upstream URL and credential/workspace headers, with capacity, idle TTL and max-age eviction. Connect/5xx handshake/stale-cache failures and send failures before `response.create` is accepted by the socket may replay through HTTP/SSE; after a successful send, read/close/1009/first-event-timeout failures terminate the lifecycle without transparent replay. The configured stream first-event timeout (default 120 seconds) covers request send, response headers, and that first valid event without being extended by SSE comments or partial bytes. After the first event, the idle timeout (default 300 seconds) only terminates the stream. Handshake 4xx and committed responses never trigger transport replay. HTTP fallback keeps the same execution/account/workspace/concurrency lease, supports flat and nested request frames, bounds one SSE event to 128 MiB and rematerializes auth after one same-account 401 refresh. SSE and WS `response.completed` events with empty output are rebuilt from prior `output_item.done`; Windows/Unix reset classification and big-frame `message_too_big` mapping are covered. Remote input images on ordinary Responses/WS/Cursor paths allow only HTTP(S) or validated data URIs, revalidate every redirect and DNS answer, pin the validated address, block private/reserved/transition IPv4/IPv6 ranges, cap 16 images and 1 MiB each with bounded concurrency/time, and require a supported MIME/signature. Dedicated Codex OAuth `/v1/images/generations` and `/v1/images/edits` bridge to the same-account Responses image tool, accept at most 16 inputs with 20 MiB per-image and 32 MiB aggregate limits, validate multipart/data-URI/remote MIME signatures and request parameters, and deliberately reject `n>1`. Explicit Responses image-tool requests, dedicated Images, and successful Grok image responses commit an SSE comment or legal JSON whitespace before long generation work and then emit 15-second heartbeats; these transport bytes do not extend upstream first-event/request deadlines. Once committed, wire HTTP status remains 200 and no transparent Provider retry is possible, so in-band errors and terminal usage status are authoritative. Missing terminal events, upstream failure, timeout, cancellation, and output bounds update terminal usage without recording Provider success; decoded image count/bytes/format/dimensions have dedicated logs and metrics. `response_format=url` uses a 256-bit, one-hour, no-store capability URL whose HTTPS host is taken only from the signed Router Share context. The default store survives restart; replicas sharing a lock-capable `CC_SWITCH_IMAGE_STORE_DIR` can serve the same URL, while independent stores require sticky routing. Capability GET/HEAD remains behind the same Router Share authentication and signed ingress. Cloudflare workers must pass `Response.body` through without buffering.",
    "- Dedicated Images wire contract: Codex OAuth image generation/edit forces upstream `stream=true` and exactly one `Accept: text/event-stream`. Every successful body is consumed as SSE regardless of a correct, missing, or incorrect upstream `Content-Type`; a raw JSON success body is a protocol error, not an alternate success mode.",
    "- Quota/subscription evidence: `/wham/usage.plan_type` is authoritative for the displayed plan. `/accounts/check` rejects expired or inactive candidates and uses exact matching for a verified workspace; `/subscriptions` is queried only for that verified workspace. Conflicting plans, untrusted workspace expiry, and past expiry contradicted by an available paid usage response are discarded, while sanitized resolution evidence is persisted for diagnostics. A discarded expiry is absent from both the auth summary and Share descriptor instead of being reported as expired. Explicit `code_review`/`codex_review`/`review` windows are exposed as separate `review_session`, `review_weekly`, or `review_monthly` tiers; malformed or empty candidates do not hide a later valid candidate, reset timestamps accept seconds or milliseconds, and review utilization does not overwrite the account's ordinary quota percentage.",
    "- Account-center and retry boundary: zero Codex OAuth accounts report `unconfigured`; one account is selected automatically; multiple accounts report `needs_selection` for account-center operations until the administrator chooses one. Selection persists only the account-center preference and never rebinds a Provider Bundle or Share. Router Share traffic uses only the Share-bound Surface and account. HTTP, SSE, Images, models, alpha search, WS, and WS→HTTP fallback never enter candidate-account selection or cross-Provider/account failover. Codex 429 bodies parse `error.resets_in_seconds` and `error.resets_at`; generic managed-account handling honors bounded `Retry-After` but writes cooldown only to the bound account.",
    "- Overflow recovery: `CC_SWITCH_CODEX_OVERFLOW_AUTO_COMPACT=1` opt-in detects HTTP 400 and pre-commit `response.failed` context overflow, summarizes bounded older input with the same Provider/account, preserves recent context and tool pairing, then retries the original request once. Summary failure degrades to an omission marker, committed output is never replayed, and summary usage is recorded separately as `codex_overflow_compact_summary`. The feature is disabled by default and never calls the top-level Router recursively.",
    "- Client gate: inbound requests reject generic tool signatures while the final outbound header pass pairs official originator/User-Agent families and raises obsolete versions before every HTTP, WebSocket, and image request.",
    "- TLS fingerprint: no Chrome/TLS impersonation is implemented in server; current stance is rustls direct TLS plus header/client gating. Real ChatGPT upstream smoke should revisit this only if upstream starts rejecting rustls traffic.",
    "",
    "### `kimi_code` (Kimi Code)",
    "",
    "Server-owned capability independently implemented from Kimi protocol evidence in OmniRoute `918fba5e392ce8b137976349f035597196edc440`, CLIProxyAPI `bd34ceca04209ef0460f4b05e3a1a047fb7fad2a`, and the earlier `claude-code-proxy` review at `4ea0414b5bce26ae38f2547a50d2564ca3d5bc1d`; it is not part of the external Provider baseline:",
    "",
    "- Device OAuth uses the fixed Kimi public client and `auth.kimi.com` endpoints with a serialized poll lease, bounded interval/expiry/body/timeouts, and explicit cancel. One generated device identity is reused by authorization, polling, the durable Account Profile, refresh, and inference.",
    "- A completed login requires access/refresh tokens plus a stable JWT `userId`; account IDs derive from that principal. Refresh rejects a changed `userId`, and a Provider rejects a stale account identity generation or missing account-scoped device identity.",
    "- Claude Kimi uses native `/coding/v1/messages?beta=true` and `/coding/v1/messages/count_tokens?beta=true`; Codex and Gemini bridge to `/coding/v1/chat/completions`; authoritative discovery uses `/coding/v1/models`. Every surface binds one explicit `kimi_code` Account and applies final Bearer plus `KimiCLI/1.37.0` and account-scoped `X-Msh-*` identity headers. Extra headers cannot override auth/device identity, and the first 401 may refresh and replay only that Account once.",
    "- Model discovery is single-flight and scoped by App, Provider revision/runtime, exact Account, auth identity generation, and token refresh generation. A successful empty catalog is authoritative; only reviewed wire models become aliases, while unknown/unreviewed models fail closed. Retryable failure can use bounded stale data only in the identical scope.",
    "- K3 effort normalizes to `low`/`high`/`max` with default `max`; thinking keeps all signed blocks. Replay is bounded and scoped by App, Provider revision/runtime, Account generation, Share, hashed signed user, session, and model family. CAS writes/deletes revalidate binding and generation; 400/422 deletes only an applied replay, and streaming commits at `message_stop` rather than waiting for EOF.",
    "- Thirty offline Kimi contract tests pass across three App surfaces, discovery, reasoning/replay, same-account 401, and generation drift. Real device login, refresh, catalog, stream/tools/images, quota, error recovery, and entitlement acceptance remain external gates. See `docs/provider/kimi-code.md`.",
    "",
    "### `qoder_cosy` (Qoder COSY)",
    "",
    "Server-owned capability independently implemented from Qoder protocol evidence in TokenRouter `a63b6b6077738d7e2222f02ec050b70d3aeb3516` and 9router `15223724c3e1ad898e84ef6e0cc1686cbafc8290`; it is not part of the external Provider baseline:",
    "",
    "- Global and China are explicit Account site capabilities with fixed audited origins. Both support bounded device login; Global additionally supports explicit `pt-*` PAT import. Login/import requires a stable Qoder principal, persists only through the Account domain write path, and never converts another Provider credential into Qoder entitlement.",
    "- Inference exchanges the exact bound Account credential into its reviewed rail, refreshes only that same Account, and fences job-token/session ownership by Provider revision/runtime plus Account auth/token generations. Session owners are single-flight and generation-checked; stale or unknown post-send outcomes fail closed rather than selecting another Account.",
    "- COSY requests use the reviewed body encoding, signing, encryption, and fixed site endpoints. Live model configuration is authoritative for enabled models and aliases; missing, disabled, site-incompatible, or unknown models fail closed. Catalog, session, and quota caches include the exact Provider/Account/site generation scope.",
    "- Claude Messages, OpenAI Responses/Chat, and Gemini inputs canonicalize into the Qoder conversation contract, while streamed/non-streamed outputs return the caller's protocol lifecycle. Text and declared tools are fixture-verified; image support remains unadvertised. Conversation identity is scoped to Share, signed user, Provider runtime, Account generations, and model rather than a global session id.",
    "- Quota exposes only reviewed COSY credit/usage evidence and keeps unavailable, unknown, stale, and supported states distinct. The public Account view exposes non-secret `site` and `credentialRail` only; PAT, access/job tokens, signing material, raw model config, and session secrets remain redacted.",
    "- Offline API, Web, signing, session, model, quota, bridge, and same-account recovery contracts are fixture-verified. Real device/PAT login, token exchange, model discovery, inference, tools, quota, expiry, and recovery remain `live_pending`; no live Qoder success is claimed.",
    "",
    "### `cursor_oauth` / `cursor_apikey` (Cursor AgentService)",
    "",
    "Cursor OAuth/API key protocol evidence review through 2026-08-23, implemented independently in Server:",
    "",
    "- OAuth/account storage: DeepControl PKCE + poll remains the browser login path; server now also imports Cursor IDE `state.vscdb` from the cc-switch-server host and falls back to cursor-agent `auth.json` across Linux/macOS/Windows (`CURSOR_AGENT_AUTH_PATH` can override). Imported IDE tokens preserve `cursorServiceMachineId`; agent auth imports are accepted without machine id. `CURSOR_STATE_DB_PATH` can override the IDE DB path; vscdb reads use an immutable SQLite URI to avoid live Cursor WAL locks; OAuth, local import, and profile enrichment derive account ids from the same WorkOS subject hash when available. Account token fields are covered by the shared encrypted `accounts.json` store.",
    "- Profile enrichment: Cursor `/api/auth/me` uses the dashboard WorkOS session cookie shape (`WorkosCursorSessionToken=<workos_user_id>::<access_token>`) derived from the access-token JWT, not the generic `Authorization: Bearer` profile request. Token exchange/refresh, poll, and profile requests now share the Cursor browser-login User-Agent. Enrichment failure is non-fatal so access-token-only imports can still be used; when profile includes `sub`/`user_id`/`id`, it is used as the stable account id seed if tokens lack a subject.",
    "- Endpoint discovery and exchange: when no rail-specific override is configured, both rails authenticate `POST https://api2.cursor.sh/aiserver.v1.ServerConfigService/GetServerConfig`, decode protobuf `AgentUrlConfig` field 27, require both `agentUrl` and `agentnUrl`, and accept only pathless, default-port HTTPS origins on `api5.cursor.sh` or its subdomains. Inference uses `agentUrl + /agent.v1.AgentService/Run`. The one-hour cache scope includes App, Provider revision, credential generation, runtime fingerprint, rail, principal, and access-token digest. API-key exchange defaults to `/auth/exchange_user_api_key`; OAuth refresh uses that endpoint with the same bound refresh token as Bearer plus `{}`. Discovery and AgentService share one same-credential 401 recovery budget, and optional endpoint overrides remain fail-closed HTTPS URLs.",
    "- Proxy transport and isolation: Claude/Codex/Gemini Cursor providers use the native HTTP/2 Connect-RPC AgentService driver by default, with provider/env settings able to disable it during incident triage. The driver covers AgentService protobuf frames, credential-bound CLI/SDK headers, KV/session handling, declared tools, images, and Anthropic/OpenAI Chat/OpenAI Responses/Gemini response formatting. Session, response, and tool-call indexes use a domain-separated typed scope containing App, Provider revision/runtime, rail/protocol revision, exact OAuth Account plus auth/token generations or API-key digest plus credential generation, Share, and normalized signed user. Reused raw ids cannot be found, replaced, closed, or resumed across scopes. The OAuth CLI rail includes W3C `traceparent`/`backend-traceparent` and detected CLI build metadata with a 60-minute cache and `cli-2026.07.08-0c04a8a` fallback; the API-key SDK rail uses the audited SDK header shape and `sdk-1.0.13` fallback.",
    "- Stream and tool boundary: first business output has one absolute deadline starting when the request is sent; response headers, partial Connect bytes, KV/context/heartbeat/tool lifecycle, and unknown control frames neither satisfy nor extend it. A fresh absolute phase begins after tool results resume a parked stream, and only a complete business frame starts idle timing. Read/write/edit/delete/list/glob/grep, diagnostics, shell/background shell, fetch, stdin, and MCP requests bridge only to a compatible declared client tool or receive a protocol-correct rejection. Surfaced call ids remain stable across pause/resume; Gemini emits `functionCall.id` and prefers `functionResponse.id`, retaining `name` only as a legacy fallback. The Server never executes arbitrary Cursor-requested filesystem or shell work.",
    "- Model modes and catalog: Share resolution fixes the Cursor Provider before parsing aliases, so model text never selects or escapes a Provider/Share. `cursor`, Agent/Plan/Ask/Composer aliases and `cursor:`, `cursor-agent:`, `cursor-plan:`, `cursor-ask:` prefixes resolve wire model/mode; arbitrary trailing `-fast` becomes the protobuf fast parameter. Explicit selectors override only the current Cursor Provider's single-model default, while ordinary unprefixed requests retain RuntimePlan policy. OAuth exposes the reviewed static aliases. API-key discovery materializes only the exact S2 Provider credential into zeroizing memory and keys token, cooldown, and model caches by App, Provider revision/runtime, credential generation, and key digest. A successful empty catalog is authoritative; only retryable transport/429/5xx failure may use the same scope's stale catalog. Gemini `/v1beta/models` uses this same exact-scope path, and non-conflicting bare IDs plus deterministic namespaced variants are collision-deduplicated.",
    "- Rate-limit hardening: AgentService 429 responses write `rateLimitedUntil` only to the Cursor Provider's explicit bound Account from `Retry-After` or Cursor JSON reset hints. Later requests remain on that binding and observe its cooldown instead of selecting another Account or Provider. Non-2xx AgentService responses read up to 8KB of JSON error detail (`error`, `message`, `code`, `details[0].message`) so clients see actionable diagnostics instead of status-only 502s.",
    "- Image boundary: Cursor's Anthropic/OpenAI Chat/OpenAI Responses/Gemini extractors use the shared case-insensitive HTTP(S)/data URI classifier. Remote loads use the shared DNS/redirect/IP/MIME/signature/count/concurrency/time limits, while native base64 branches reject payloads above the 1 MiB decoded bound before allocating their decoded buffer.",
    "- Evidence: `cargo test cursor` passes 197 unit tests plus one API contract test (keyword-filtered, so the count includes pagination-cursor tests). The relevant contracts cover endpoint discovery/trust, exact-scope sessions/indexes, initial and resumed absolute deadlines, builtin rejection, credentials/catalogs, all four response emitters, and signed Gemini Share catalog isolation. Maturity remains Experimental. Do not mark live Cursor OAuth/API-key proxy acceptance complete until a real Cursor account has exercised discovery, streaming, tool call/result continuation, images, builtin/declared tools, and rate-limit/cooldown behavior.",
    "",
    "### `grok_oauth` (Grok/xAI OAuth)",
    "",
    "Server-owned capability based on protocol evidence reviewed through 2026-08-13; it is not part of the external Provider baseline:",
    "",
    "- OAuth/account storage: xAI public client id, PKCE, `plan=generic`, `referrer=cc-switch-server`, workspace read/write scopes, browser nonce validation, serialized device polling, and strict ES256 OIDC/JWKS verification with an EC P-256 signing key. Device start/poll advertise the shared CLI version plus `x-grok-client-surface: ui`; production authorize/token/discovery/JWKS endpoints are fixed to audited `auth.x.ai` HTTPS URLs, while loopback injection is test-only. Native refresh accepts an omitted replacement ID token only for an account with an existing verified subject, verifies any new ID token, and rejects subject changes. Explicit `~/.grok/auth.json` import also requires a signed ID token.",
    "- Proxy headers/body: OpenAI Responses upstream contract, `Authorization: Bearer`, `x-grok-conv-id`, Grok CLI identity defaulting to `0.2.111`, authoritative single-model routing with editable `grok-4.5` default, Responses field cleanup, reasoning effort/model/tool guards, and `encrypted_content` shape validation. `x-grok-turn-idx` is forwarded only from a valid downstream decimal u64; the server never fabricates or increments it, and the same optional value survives same-account 401 replay and WS→HTTP fallback.",
    "- Hosted search translation: Grok `web_search_call` and hosted `x_search` become Anthropic `server_tool_use` plus matching search-result blocks in streaming and snapshots. Item-id correlation covers events without `output_index`; URL annotations become citations; usage records total hosted web searches and the X-search subset. Hosted searches do not produce a client `tool_use` stop reason.",
    "- Single-account retry boundary: every Grok Provider binds one concrete OAuth account. Initial authentication resolution, HTTP/JSON, SSE, media, WebSocket handshake, and WS→HTTP fallback use the verified Share binding and never rotate accounts or enter generic cross-Provider failover; same-account 401 handling may force-refresh only that account once. `/v1/models` is also Share-scoped and cannot select an arbitrary Grok Provider. Credential persistence degradation returns 503 before Grok data-plane traffic.",
    "- Media/WS/models: Grok images/videos routes forward to `api.x.ai/v1`; image edits translate common OpenAI multipart uploads to xAI JSON data URLs; Responses GET bridges to `wss://api.x.ai/v1/responses`. Models, HTTP, media, and WebSocket inference routes require verified Router ingress with a Share identity; unsigned requests and signed client-lane requests without a Share are rejected. WS/image/edit/video/search/media-entitlement capabilities default fail-closed and use explicit current-generation evidence with TTL; legacy profile flags cannot authorize access. Video task ownership is durably bound to Provider, exact Account, `authIdentityGeneration`, Share/runtime/user namespace and TTL, with bounded startup/backup validation and no cross-account resolution. Model discovery uses a bounded ETag/TTL cache, last-known-good fallback, and exposes `source`, `stale`, and `fetchedAtMs`. Loopback WS/model endpoint injection exists only in test builds.",
    "- Reconciliation: the existing Provider test surface returns a Grok-only structured report with binding status, planned/remaining credential action, identity and token generations, endpoint policy, Responses/model probe state, and non-secret capability evidence. Dry-run is local and read-only. Network mode may generation-fenced refresh only the exact bound Account and then probe only its Provider; refresh failure is redacted and stops before Responses. Model drift requires an explicit structured model rejection, while auth, rate-limit, ambiguous 4xx and 5xx remain inconclusive. Reconciliation never scans, selects, mutates, or fails over to another account.",
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
  assertRequiredProviderCoverage(providerRegistry);
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
const mdPath = path.join(repoRoot, "docs/provider/coverage.md");
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
