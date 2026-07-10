#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const presetSourceRoot =
  process.env.CC_SWITCH_SOURCE_DIR ||
  process.env.CC_SWITCH_UPSTREAM_DIR ||
  "/data/projects/cc-switch";
const providerTypeSourceRoot =
  process.env.CC_SWITCH_PROVIDER_TYPE_SOURCE_DIR || "/data/projects/cc-switch";
const checkMode = process.argv.includes("--check");

const providerTypeSource = path.join(
  providerTypeSourceRoot,
  "src-tauri/src/proxy/providers/mod.rs",
);

const presetFiles = {
  claude: path.join(presetSourceRoot, "src/config/claudeProviderPresets.ts"),
  codex: path.join(presetSourceRoot, "src/config/codexProviderPresets.ts"),
  gemini: path.join(presetSourceRoot, "src/config/geminiProviderPresets.ts"),
  universal: path.join(presetSourceRoot, "src/config/universalProviderPresets.ts"),
};

const requiredProviderTypes = [
  ["claude", "Anthropic official / API key", ["claude"]],
  ["claude_auth", "Claude bearer-only relay", ["claude"]],
  ["claude_oauth", "Claude Official OAuth", ["claude"]],
  ["codex", "OpenAI/Codex compatible", ["codex"]],
  ["codex_oauth", "OpenAI ChatGPT OAuth", ["claude", "codex"]],
  ["gemini", "Google Gemini API key", ["gemini"]],
  ["gemini_cli", "Google Gemini OAuth / CLI", ["gemini", "claude"]],
  ["openrouter", "OpenRouter", ["claude", "codex", "gemini"]],
  ["github_copilot", "GitHub Copilot", ["claude"]],
  ["deepseek_account", "DeepSeek account", ["claude"]],
  ["kiro_oauth", "Kiro OAuth", ["claude"]],
  ["cursor_oauth", "Cursor OAuth", ["claude", "codex"]],
  ["cursor_apikey", "Cursor API key", ["claude", "codex"]],
  ["antigravity_oauth", "Antigravity OAuth", ["claude", "gemini"]],
  ["agy_oauth", "Antigravity CLI / agy", ["claude", "gemini"]],
  ["ollama_cloud", "Ollama API key", ["claude", "codex"]],
];

const serverCompatibilityProviderTypes = [
  ["aws_bedrock", "AWS Bedrock compatibility schema", ["claude"]],
  ["nvidia", "Nvidia OpenAI-compatible API", ["claude", "codex"]],
  ["deepseek_api", "DeepSeek API key", ["claude", "codex"]],
  ["grok_oauth", "Grok/xAI OAuth reverse proxy", ["claude", "codex", "gemini"]],
];

function read(file) {
  return fs.readFileSync(file, "utf8");
}

function extractProviderTypeIds() {
  const source = read(providerTypeSource);
  const body = source.match(/pub enum ProviderType \{([\s\S]*?)\n\}/)?.[1] ?? "";
  const variants = [...body.matchAll(/^\s*([A-Z][A-Za-z0-9]*)\s*,/gm)].map(
    (match) => match[1],
  );

  const asStrBody = source.match(/pub fn as_str\(&self\).*?\{([\s\S]*?)\n    \}/)?.[1] ?? "";
  const ids = new Map();
  for (const match of asStrBody.matchAll(/ProviderType::([A-Za-z0-9]+)\s*=>\s*"([^"]+)"/g)) {
    ids.set(match[1], match[2]);
  }

  return variants.map((variant) => ids.get(variant)).filter(Boolean);
}

function extractPresets(file) {
  const source = read(file);
  const arrayStart = source.indexOf("= [");
  const start = source.indexOf("[", arrayStart >= 0 ? arrayStart : 0);
  if (start < 0) return [];

  const presets = [];
  for (const body of topLevelObjects(source.slice(start))) {
    const name = body.match(/^\s*\{\s*name:\s*"([^"]+)"/)?.[1];
    if (!name) continue;
    const providerType = body.match(/providerType:\s*"([^"]+)"/)?.[1] ?? null;
    const apiFormat = body.match(/apiFormat:\s*"([^"]+)"/)?.[1] ?? null;
    const baseUrl = extractBaseUrl(body);
    presets.push({
      name,
      providerType,
      apiFormat,
      baseUrl,
    });
  }
  return dedupePresets(presets);
}

function extractBaseUrl(body) {
  const directBaseUrl = body.match(/baseURL:\s*"([^"]+)"/)?.[1];
  if (directBaseUrl) return directBaseUrl;

  const envBaseUrl = body.match(
    /(?:ANTHROPIC_BASE_URL|GOOGLE_GEMINI_BASE_URL|GEMINI_BASE_URL):\s*"([^"]+)"/,
  )?.[1];
  if (envBaseUrl) return envBaseUrl;

  const codexGeneratedBaseUrl = body.match(
    /generateThirdPartyConfig\(\s*"[^"]+"\s*,\s*"([^"]+)"/,
  )?.[1];
  if (codexGeneratedBaseUrl) return codexGeneratedBaseUrl;

  return null;
}

function topLevelObjects(input) {
  const objects = [];
  let depth = 0;
  let start = -1;
  let inString = false;
  let quote = "";
  let escaped = false;

  for (let i = 0; i < input.length; i += 1) {
    const char = input[i];
    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === quote) {
        inString = false;
      }
      continue;
    }

    if (char === '"' || char === "'" || char === "`") {
      inString = true;
      quote = char;
      continue;
    }

    if (char === "{") {
      if (depth === 0) start = i;
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0 && start >= 0) {
        objects.push(input.slice(start, i + 1));
        start = -1;
      }
    } else if (char === "]" && depth === 0) {
      break;
    }
  }
  return objects;
}

function dedupePresets(items) {
  const seen = new Set();
  const result = [];
  for (const item of items) {
    const key = `${item.name}\u0000${item.providerType ?? ""}`;
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(item);
  }
  return result;
}

function buildCoverage() {
  const sourceProviderTypes = new Set(extractProviderTypeIds());
  const providerTypes = requiredProviderTypes.map(([id, label, apps]) => ({
    id,
    label,
    apps,
    required: true,
    presentInSource: sourceProviderTypes.has(id),
  }));
  providerTypes.push(
    ...serverCompatibilityProviderTypes.map(([id, label, apps]) => ({
      id,
      label,
      apps,
      required: false,
      presentInSource: false,
    })),
  );

  return {
    generatedFrom: {
      providerTypes: providerTypeSourceRoot,
      presets: presetSourceRoot,
    },
    providerTypes,
    presets: {
      claude: extractPresets(presetFiles.claude),
      codex: extractPresets(presetFiles.codex),
      gemini: extractPresets(presetFiles.gemini),
      universal: extractPresets(presetFiles.universal),
    },
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
      ["codex_oauth", "grok_oauth", "cursor_oauth", "cursor_apikey", "ollama_cloud"].includes(
        preset.providerType,
      )
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
    if (["antigravity_oauth", "agy_oauth", "grok_oauth"].includes(preset.providerType)) {
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
  if (typeof coverage.generatedFrom === "string") {
    lines.push(`Generated from: \`${coverage.generatedFrom}\``);
  } else {
    lines.push(`Provider types from: \`${coverage.generatedFrom.providerTypes}\``);
    lines.push(`Presets from: \`${coverage.generatedFrom.presets}\``);
  }
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
  for (const key of ["claude", "codex", "gemini", "universal"]) {
    lines.push(`## ${key} presets`);
    lines.push("");
    lines.push("| Name | providerType |");
    lines.push("| --- | --- |");
    for (const preset of coverage.presets[key]) {
      lines.push(`| ${preset.name} | ${preset.providerType ? `\`${preset.providerType}\`` : ""} |`);
    }
    lines.push("");
  }
  lines.push(...serverParityNotes());
  return `${lines.join("\n").trimEnd()}\n`;
}

function serverParityNotes() {
  return [
    "## Server parity notes",
    "",
    "### `kiro_oauth` (Kiro OAuth)",
    "",
    "Server-native Kiro pass from `/data/projects/proxy/Kiro/Kiro.md` P0-P2 (2026-07-10):",
    "",
    "- OAuth/account storage: Builder ID and IdC device flow share AWS SSO OIDC registration, `issuerUrl` is persisted for IdC re-registration, and Google/GitHub Social login uses Kiro's server-safe device authorization/poll endpoints. Native refresh is selected dynamically by `authMethod` for Builder ID/IdC/Social/External IdP; OIDC refresh 401 can re-register the client and retry once.",
    "- Imports: Kiro `credentials.json` can be pasted or read from the server host, and `ksk_` API keys are validated through `ListAvailableProfiles` before import. The account store recursively encrypts token/API-key/client-secret fields, including nested refresh responses.",
    "- Proxy: Claude-only Kiro forwarding builds CodeWhisperer IDE requests by default and can use the CLI endpoint when account metadata sets `endpoint=cli`; requests add API_KEY/EXTERNAL_IDP `tokentype` when needed, default `profileArn` by auth method, and fall back to profileArn-derived region. EventStream parsing now validates prelude/message CRC and inline `<thinking>` content is split into Claude reasoning blocks.",
    "- Quota: `getUsageLimits` is available through the normal quota refresh path and refresh updates can backfill `kiroUsageLimits`.",
    "- Real Kiro upstream validation remains an external gate: do not mark Kiro native acceptance complete until a real Kiro account has exercised Claude non-stream, stream, usage refresh, refresh-token rollover, and failover.",
    "",
    "### `claude_oauth` (Claude Official)",
    "",
    "Server-native Claude OAuth proxy parity pass from `/data/projects/proxy/Claude/CLAUDE.md` P0/P1 plus second/third/fourth/fifth-round hardening (2026-07-10):",
    "",
    "- Proxy hot path: `?beta=true`, request-shape-driven `anthropic-beta` assembly (`claude-code-20250219`, `oauth-2025-04-20`, plus thinking/tool/computer-use betas only when needed), Claude CLI header set, per-account stable stainless OS/arch profile, stream-sensitive `x-stainless-timeout`, first-user-text-seeded `x-claude-code-session-id`, `metadata.user_id`, billing/identity `system` injection with billing block dedupe, default `tools: []`, preserve-order JSON serialization, `cch=` body signing, env-overridable CCH seed, and `cc_entrypoint=cli` CCH default (`src/proxy/claude_oauth.rs`, `src/domain/claude_cli.rs`).",
    "- Retry hardening: Claude OAuth streams pre-read the first upstream chunk; first-chunk `event:error` records breaker outcome and retries within a 3-attempt/10s budget. Non-streaming 400 signature/thinking failures trigger reactive body retry stages: thinking blocks become text, tool blocks can be downgraded to text on tool-signature errors, and web_search history blocks are stripped as a final fallback.",
    "- Operations hardening: the quota refresh loop first warm-refreshes due native OAuth tokens, Claude OAuth accounts use per-account in-flight guards (default 8, provider/env configurable) and least-utilized selection while preserving failover queue tie-breaks, non-streaming version-gate responses are rewritten into admin-facing guidance to bump `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA`, and upstream `x-request-id` is passed through for support correlation.",
    "- OAuth web-paste: `code#state` parsing, platform token endpoint first, platform User-Agent (`axios/1.13.6`).",
    "- Local callback uses `/api/accounts/login/callback`; Claude CLI callback route `/web-api/oauth/claude-cli/callback` is also registered, while a dedicated `127.0.0.1:54547` listener remains a deployment/productization choice.",
    "",
    "### `codex_oauth` (OpenAI OAuth)",
    "",
    "Server-native Codex/OpenAI OAuth proxy parity pass from `/data/projects/proxy/Codex/Codex.md` P0-P2 (2026-07-09):",
    "",
    "- OAuth/account storage: CLI callback route `/web-api/oauth/openai-cli/callback`, per-account refresh serialization, token fields encrypted in `accounts.json` with `accounts.key` included in backup targets, and JWT/profile-derived fields `chatgpt_account_id`, `chatgptAccountId`, `planType`, `poid`, `organizations`, `subscription.expiresAt`.",
    "- Proxy headers/body: managed account requests inject `chatgpt-account-id`, `originator=codex_cli_rs`, `version=0.125.0`, Codex CLI User-Agent, session/window headers, `reasoning.encrypted_content`, `prompt_cache_key`, and versioned Codex instructions templates; ordinary `/v1/responses` requests carrying `input[].type=compaction_trigger` are promoted to `/responses/compact`.",
    "- Streaming/WS/images: Responses POST SSE keeps existing protocol conversion; Responses GET upgrades through WebSocket to `wss://chatgpt.com/backend-api/codex/responses` with `openai-beta: responses_websockets=2026-02-06` and maps big-frame failures to `message_too_big`; stream `response.completed` events with missing/empty `response.output` are patched from prior `response.output_item.done` events; unnamed pending function-call starts are delayed until `done` supplies the tool name; `/v1/images/generations` dispatches to Grok OAuth media or Codex OAuth image-generation bridge when enabled.",
    "- Rate limits/failover: 429 bodies parse `error.resets_in_seconds` and `error.resets_at`, write account `rateLimitedUntil`, and provider selection skips cooling-down Codex OAuth accounts; explicit provider selection returns 429 while cooling down.",
    "- Client gate: Codex OAuth upstream calls reject generic tool signatures when `originator` is present and require a Codex-compatible originator plus official UA engine shape. Requests without `originator` remain allowed except obvious generic tools, to avoid breaking older official clients.",
    "- TLS fingerprint: no Chrome/TLS impersonation is implemented in server; current stance is rustls direct TLS plus header/client gating. Real ChatGPT upstream smoke should revisit this only if upstream starts rejecting rustls traffic.",
    "",
    "### `cursor_oauth` / `cursor_apikey` (Cursor AgentService)",
    "",
    "Server-native Cursor OAuth/API key proxy parity pass from `/data/projects/proxy/Cursor/Cursor.md` P0-P2 (2026-07-09):",
    "",
    "- OAuth/account storage: DeepControl PKCE + poll remains the browser login path; server now also imports Cursor IDE `state.vscdb` from the cc-switch-server host and falls back to cursor-agent `auth.json` across Linux/macOS/Windows (`CURSOR_AGENT_AUTH_PATH` can override). Imported IDE tokens preserve `cursorServiceMachineId`; agent auth imports are accepted without machine id. `CURSOR_STATE_DB_PATH` can override the IDE DB path; vscdb reads use an immutable SQLite URI to avoid live Cursor WAL locks; OAuth, local import, and profile enrichment derive account ids from the same WorkOS subject hash when available. Account token fields are covered by the shared encrypted `accounts.json` store.",
    "- Profile enrichment: Cursor `/api/auth/me` uses the dashboard WorkOS session cookie shape (`WorkosCursorSessionToken=<workos_user_id>::<access_token>`) derived from the access-token JWT, not the generic `Authorization: Bearer` profile request. Token exchange/refresh, poll, and profile requests now share the Cursor browser-login User-Agent. Enrichment failure is non-fatal so access-token-only imports can still be used; when profile includes `sub`/`user_id`/`id`, it is used as the stable account id seed if tokens lack a subject.",
    "- Proxy transport: Claude/Codex/Gemini Cursor providers use the native HTTP/2 Connect-RPC AgentService driver by default, with provider/env settings able to disable it during incident triage. The driver covers AgentService protobuf frames, cursor-agent CLI headers, KV/session handling, built-in tool rejection, declared tools, images, and Anthropic/OpenAI Chat/OpenAI Responses/Gemini response formatting. AgentService headers include W3C `traceparent`/`backend-traceparent`; timezone comes from `TZ`; client version is detected from local Cursor state with a 60-minute cache and falls back to `cli-2026.01.09-231024f`.",
    "- Error/failover hardening: AgentService 429 responses now write account `rateLimitedUntil` from `Retry-After` or Cursor JSON reset hints, and provider selection skips cooling Cursor accounts. Non-2xx AgentService responses read up to 8KB of JSON error detail (`error`, `message`, `code`, `details[0].message`) so clients see actionable diagnostics instead of status-only 502s.",
    "- Already present in the current tree: OmniRoute-derived `TOOL_COMMIT_DIRECTIVE`, CLI-minimal AgentService headers, 1MB image size limit plus private/link-local IP and `.internal`/`.local`/`.lan` host blocking, and per-account provider binding.",
    "- Real Cursor upstream validation remains an external gate: do not mark live Cursor OAuth/API key proxy acceptance complete until a real Cursor account has exercised streaming, tool call/result continuation, images, and failover.",
    "",
    "### `grok_oauth` (Grok/xAI OAuth)",
    "",
    "Server-only capability from `/data/projects/proxy/Grok/Grok.md` P0-P2 (2026-07-09); not a desktop upstream provider coverage debt:",
    "",
    "- OAuth/account storage: xAI public client id, PKCE, `plan=generic`, `referrer=cc-switch-server`, endpoint allowlist for `x.ai`/`*.x.ai`, JWT-derived profile fields, native refresh, and explicit `~/.grok/auth.json` import.",
    "- Proxy headers/body: OpenAI Responses upstream contract, `Authorization: Bearer`, `x-grok-conv-id`, model mapping to `grok-4.3`, Responses field cleanup, reasoning effort model allowlist, tool allowlist, and `encrypted_content` shape guard.",
    "- Media/WS: Grok images/videos routes forward to `api.x.ai/v1`; image edits translate common OpenAI multipart uploads to xAI JSON data URLs; Responses GET can bridge to `wss://api.x.ai/v1/responses`.",
    "- Rate limits/failover: 401/403/429/5xx responses write account cooldown and provider selection skips cooling-down Grok accounts.",
    "",
    ];
}

function assertCoverage(coverage) {
  const missingTypes = coverage.providerTypes
    .filter((item) => item.required && !item.presentInSource)
    .map((item) => item.id);
  if (missingTypes.length > 0) {
    throw new Error(`Missing provider types in source: ${missingTypes.join(", ")}`);
  }
  for (const key of ["claude", "codex", "gemini"]) {
    if (coverage.presets[key].length === 0) {
      throw new Error(`No ${key} presets extracted`);
    }
  }
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
  claude: coverage.presets.claude.map((preset) => providerFixture("claude", preset)),
  codex: coverage.presets.codex.map((preset) => providerFixture("codex", preset)),
  gemini: coverage.presets.gemini.map((preset) => providerFixture("gemini", preset)),
};

const jsonPath = path.join(repoRoot, "assets/contract/provider-coverage.json");
const mdPath = path.join(repoRoot, "docs/provider-coverage.md");
const json = `${JSON.stringify(coverage, null, 2)}\n`;
const markdown = toMarkdown(coverage);

if (checkMode) {
  const actualJson = fs.existsSync(jsonPath) ? fs.readFileSync(jsonPath, "utf8") : "";
  const actualMd = fs.existsSync(mdPath) ? fs.readFileSync(mdPath, "utf8") : "";
  if (actualJson !== json || actualMd !== markdown) {
    throw new Error("provider coverage assets/docs are out of date; run scripts/audit/audit-provider-coverage.mjs");
  }
  console.log("provider coverage assets/docs are up to date");
} else {
  const changed =
    writeIfChanged(jsonPath, json) | writeIfChanged(mdPath, markdown);
  console.log(changed ? "provider coverage assets/docs updated" : "provider coverage assets/docs unchanged");
}
