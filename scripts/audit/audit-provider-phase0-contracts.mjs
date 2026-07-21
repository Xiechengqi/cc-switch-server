#!/usr/bin/env node
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const routerRoot = process.env.CC_SWITCH_ROUTER_ROOT || "/data/projects/cc-switch-router";
const serverBaselineCommit = "90329f4a4681552ca85e48a107c7e1fc67466dd0";
const routerBaselineCommit = "43ebea0ea20f7ab8be081d929c4fdd7cf79a40b1";
const checkMode = process.argv.includes("--check");

const contractPaths = Object.freeze({
  fields: "assets/contract/provider-field-consumption.json",
  behavior: "assets/contract/provider-legacy-behavior.json",
  writers: "assets/contract/provider-writer-inventory.json",
  compatibility: "assets/contract/provider-compatibility-window.json",
  router: "assets/contract/router-provider-channel-baseline.json",
});

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(repoRoot, relativePath), "utf8"));
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function gitShow(repository, commit, relativePath) {
  return execFileSync("git", ["-C", repository, "show", `${commit}:${relativePath}`], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function pinnedSources(repository, commit, paths) {
  const resolved = execFileSync("git", ["-C", repository, "rev-parse", commit], {
    encoding: "utf8",
  }).trim();
  if (resolved !== commit) throw new Error(`baseline commit must be full: ${commit}`);
  return paths.map((relativePath) => {
    const source = gitShow(repository, commit, relativePath);
    return { path: relativePath, sha256: sha256(source) };
  });
}

const presentationRoots = new Set([
  "apiKeyUrl",
  "category",
  "description",
  "icon",
  "iconColor",
  "isOfficial",
  "name",
  "partnerPromotionKey",
  "theme",
  "websiteUrl",
]);
const identityRoots = new Set(["providerType", "requiresOAuth"]);
const runtimeRoots = new Set([
  "apiFormat",
  "baseURL",
  "codexChatReasoning",
  "config",
  "endpointCandidates",
  "model",
  "modelMapping",
  "settingsConfig",
]);
const observationRoots = new Set(["modelCatalog", "modelsUrl"]);
const operationRoots = new Set(["testConfig"]);
const secretNames = new Set([
  "ANTHROPIC_API_KEY",
  "ANTHROPIC_AUTH_TOKEN",
  "AWS_ACCESS_KEY_ID",
  "AWS_SECRET_ACCESS_KEY",
  "OPENAI_API_KEY",
]);

export function classifyPresetPointer(pointer) {
  const segments = pointer.split("/").filter(Boolean);
  const root = segments[0];
  if (!root) throw new Error(`invalid preset pointer: ${pointer}`);
  if (root === "auth") return { classification: "credential", secret: true };
  if (root === "templateValues") {
    const field = segments[1];
    const leaf = segments.at(-1);
    if (leaf === "label" || leaf === "placeholder") {
      return { classification: "presentation", secret: false };
    }
    if (leaf === "editorValue" && field !== "AWS_REGION") {
      return { classification: "credential_template", secret: true };
    }
    return { classification: "runtime_template", secret: false };
  }
  if (
    segments.some((segment) => secretNames.has(segment)) ||
    segments.includes("apiKey")
  ) {
    return { classification: "credential", secret: true };
  }
  if (presentationRoots.has(root)) return { classification: "presentation", secret: false };
  if (identityRoots.has(root)) return { classification: "legacy_identity", secret: false };
  if (runtimeRoots.has(root)) return { classification: "runtime", secret: false };
  if (observationRoots.has(root)) return { classification: "observation", secret: false };
  if (operationRoots.has(root)) return { classification: "operation", secret: false };
  throw new Error(`unclassified preset pointer: ${pointer}`);
}

function presetFieldEntries(inventory) {
  const entries = new Map();
  for (const [app, presets] of Object.entries(inventory.presets)) {
    for (const preset of presets) {
      for (const pointer of preset.declaredPointers) {
        const key = `${app}:${pointer}`;
        const classification = classifyPresetPointer(pointer);
        const existing = entries.get(key) ?? {
          locator: `legacyPreset.${app}${pointer}`,
          sourceFormat: "json_pointer",
          ...classification,
          presets: [],
          reader: ["web-src/src/components/providers/forms/ProviderForm.tsx"],
          writer: ["web-src/src/components/providers/forms/ProviderForm.tsx::performSubmit"],
          targetOwner:
            classification.secret
              ? "credentials"
              : classification.classification === "presentation"
                ? "profile_presentation"
                : classification.classification === "observation"
                  ? "registry_or_observation"
                  : classification.classification === "operation"
                    ? "operation_policy"
                    : classification.classification === "legacy_identity"
                      ? "profile_registry"
                      : "runtime_config",
          migration: classification.secret ? "credential_patch_slot" : "profile_specific_s1_extractor",
          retirePhase: classification.classification === "presentation" ? "phase-2" : "phase-10",
        };
        existing.presets.push(preset.name);
        entries.set(key, existing);
      }
    }
  }
  return [...entries.values()].map((entry) => ({
    ...entry,
    presets: [...new Set(entry.presets)].sort(),
  }));
}

function persistedField(locator, classification, options = {}) {
  const secret = options.secret === true;
  return {
    locator,
    sourceFormat: options.sourceFormat ?? "json_pointer",
    classification,
    secret,
    reader: options.reader ?? ["src/domain/providers/model.rs", "src/proxy/adapters.rs"],
    writer: options.writer ?? [
      "src/api/providers.rs",
      "src/api/invoke/dispatch.rs",
      "web-src/src/components/providers/forms/ProviderForm.tsx",
    ],
    targetOwner:
      options.targetOwner ??
      (secret
        ? "credentials"
        : classification === "presentation"
          ? "display"
          : classification === "legacy_identity"
            ? "profile_registry"
            : classification === "observation"
              ? "observation"
              : classification === "operation"
                ? "operation_policy"
                : "runtime_config"),
    migration: options.migration ?? (secret ? "credential_patch_slot" : "typed_s1_extractor"),
    retirePhase: options.retirePhase ?? "phase-10",
    disposition: options.disposition ?? "migrate",
  };
}

function buildFieldConsumption(inventory) {
  const envRuntimeKeys = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
    "ANTHROPIC_MODEL",
    "AUTH_MODE",
    "AWS_REGION",
    "BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CODEX_BASE_URL",
    "CODEX_MODEL",
    "GEMINI_BASE_URL",
    "GEMINI_MODEL",
    "GOOGLE_GEMINI_BASE_URL",
    "OPENAI_BASE_URL",
    "OPENAI_MODEL",
  ];
  const envSecretKeys = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "CODEX_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GROK_API_KEY",
    "OPENAI_API_KEY",
    "XAI_API_KEY",
  ];
  const fields = [
    persistedField("provider.id", "resource_identity", { targetOwner: "provider_key", retirePhase: "never" }),
    persistedField("provider.name", "presentation", { retirePhase: "never" }),
    persistedField("provider.category", "presentation", { retirePhase: "phase-2" }),
    persistedField("provider.settingsConfig", "runtime_container"),
    persistedField("provider.settingsConfig.auth_mode", "runtime"),
    persistedField("provider.settingsConfig.apiFormat", "runtime"),
    persistedField("provider.settingsConfig.api_format", "runtime"),
    persistedField("provider.settingsConfig.model", "runtime"),
    persistedField("provider.settingsConfig.modelMapping.mode", "runtime"),
    persistedField("provider.settingsConfig.modelMapping.upstreamModel", "runtime"),
    persistedField("provider.settingsConfig.model_mapping.mode", "runtime"),
    persistedField("provider.settingsConfig.model_mapping.upstream_model", "runtime"),
    persistedField("provider.settingsConfig.models[*]", "runtime"),
    persistedField("provider.settingsConfig.modelCatalog", "observation"),
    persistedField("provider.settingsConfig.modelCatalog.*", "observation"),
    persistedField("provider.settingsConfig.testConfig", "operation"),
    persistedField("provider.settingsConfig.testConfig.testModel", "operation"),
    persistedField("provider.settingsConfig.testConfig.model", "operation"),
    persistedField("provider.settingsConfig.auth.OPENAI_API_KEY", "credential", { secret: true }),
    persistedField("provider.settingsConfig.apiKey", "credential", { secret: true }),
    ...envRuntimeKeys.map((key) =>
      persistedField(`provider.settingsConfig.env.${key}`, "runtime"),
    ),
    ...envSecretKeys.map((key) =>
      persistedField(`provider.settingsConfig.env.${key}`, "credential", { secret: true }),
    ),
    persistedField("provider.meta", "legacy_container"),
    persistedField("provider.meta.providerType", "legacy_identity"),
    persistedField("provider.meta.apiFormat", "runtime"),
    persistedField("provider.meta.authBinding.source", "account_binding"),
    persistedField("provider.meta.authBinding.authProvider", "account_binding"),
    persistedField("provider.meta.authBinding.accountId", "account_binding"),
    persistedField("provider.meta.githubAccountId", "account_binding"),
    persistedField("provider.meta.apiKeyField", "credential_locator", { targetOwner: "credentials" }),
    persistedField("provider.meta.customUserAgent", "runtime"),
    persistedField("provider.meta.isFullUrl", "runtime"),
    persistedField("provider.meta.promptCacheKey", "runtime"),
    persistedField("provider.meta.codexFastMode", "runtime"),
    persistedField("provider.meta.codexImageGenerationEnabled", "runtime"),
    persistedField("provider.meta.codexImageToolStripPolicy", "runtime"),
    persistedField("provider.meta.codexWebsocketEnabled", "runtime"),
    persistedField("provider.meta.codexChatReasoning", "runtime"),
    persistedField("provider.meta.localProxyRequestOverrides", "runtime", {
      disposition: "retire_unless_consumer_is_allowlisted",
      migration: "block_s2_until_typed",
    }),
    persistedField("provider.meta.testConfig", "operation"),
    persistedField("provider.meta.costMultiplier", "control_policy"),
    persistedField("provider.meta.pricingModelSource", "control_policy"),
    persistedField("provider.meta.quotaDispatchLimitPercent", "control_policy"),
    persistedField("provider.meta.commonConfigEnabled", "legacy_ui", { disposition: "retire" }),
    persistedField("provider.meta.custom_endpoints.*", "observation", { disposition: "retire" }),
    persistedField("provider.meta.endpointAutoSelect", "observation", { disposition: "retire" }),
    persistedField("provider.meta.claudeDesktopMode", "non_target", { disposition: "retire" }),
    persistedField("provider.meta.claudeDesktopModelRoutes.*", "non_target", { disposition: "retire" }),
    persistedField("provider.meta.usage_script", "non_target", { disposition: "retire" }),
    persistedField("provider.meta.isPartner", "presentation"),
    persistedField("provider.meta.partnerPromotionKey", "presentation"),
    persistedField("provider.extra.sortIndex", "presentation_order", { targetOwner: "order_by_app", retirePhase: "phase-3" }),
    persistedField("provider.extra.websiteUrl", "presentation"),
    persistedField("provider.extra.notes", "presentation"),
    persistedField("provider.extra.icon", "presentation"),
    persistedField("provider.extra.iconColor", "presentation"),
    persistedField("provider.extra.*", "unknown_flatten", {
      targetOwner: "legacy_compat",
      migration: "block_s2_until_classified",
      disposition: "blocker",
    }),
    persistedField("provider.meta.extra.*", "unknown_flatten", {
      targetOwner: "legacy_compat",
      migration: "block_s2_until_classified",
      disposition: "blocker",
    }),
  ];

  const tomlFields = [
    ["model", "runtime", false],
    ["model_provider", "legacy_identity", false],
    ["model_reasoning_effort", "runtime", false],
    ["disable_response_storage", "runtime", false],
    ["model_providers.<id>.name", "presentation", false],
    ["model_providers.<id>.base_url", "runtime", false],
    ["model_providers.<id>.wire_api", "runtime", false],
    ["model_providers.<id>.requires_openai_auth", "credential_locator", false],
    ["model_providers.<id>.env_key", "credential_locator", false],
    ["model_providers.<id>.http_headers.<name>", "credential", true],
    ["model_providers.<id>.query_params.<name>", "credential", true],
  ].map(([key, classification, secret]) =>
    persistedField(`provider.settingsConfig.config#toml:${key}`, classification, {
      sourceFormat: "toml_typed_key",
      secret,
      reader: [
        "src/domain/providers/model.rs::extract_codex_toml_base_url",
        "src/domain/providers/model_routing.rs::extract_codex_toml_model",
        "src/proxy/mod.rs::codex_provider_api_key",
        "src/proxy/adapters.rs::codex_config_base_url",
      ],
      migration: "parse_with_smol_toml_then_extract_typed_key",
    }),
  );

  return {
    schemaVersion: 1,
    authority: "phase-0-migration-ledger",
    serverBaselineCommit,
    sourceEvidence: pinnedSources(repoRoot, serverBaselineCommit, [
      "src/domain/providers/model.rs",
      "src/domain/providers/model_routing.rs",
      "src/domain/providers/store.rs",
      "src/proxy/mod.rs",
      "src/proxy/adapters.rs",
      "web-src/src/components/providers/forms/ProviderForm.tsx",
    ]),
    presetFields: presetFieldEntries(inventory),
    persistedFields: [...fields, ...tomlFields],
    dynamicPolicies: [
      {
        locator: "provider.settingsConfig.env.*",
        keyPolicy: "known keys are enumerated above; unknown keys are not silently migrated",
        valuePolicy: "string only; unknown key names matching key|token|secret|password are secret",
        unknownDisposition: "block_s2_until_classified",
      },
      {
        locator: "provider.settingsConfig.modelCatalog.*",
        keyPolicy: "model id string",
        valuePolicy: "bounded model observation object",
        unknownDisposition: "preserve_as_observation_not_runtime",
      },
      {
        locator: "provider.meta.custom_endpoints.*",
        keyPolicy: "legacy endpoint name",
        valuePolicy: "legacy observation only",
        unknownDisposition: "retire_after_typed_endpoint_policy",
      },
      {
        locator: "provider.extra.* / provider.meta.extra.*",
        keyPolicy: "flattened unknown key",
        valuePolicy: "read-only legacy payload; may contain runtime or secret material",
        unknownDisposition: "block_s2_and_adoption",
      },
    ],
  };
}

const profileByPreset = Object.freeze({
  "claude:Claude Official": "claude.official_oauth",
  "claude:OpenAI OAuth": "claude.openai_oauth",
  "claude:Grok OAuth": "claude.grok_oauth",
  "claude:Kiro OAuth": "claude.kiro_oauth",
  "claude:Ollama API Key": "claude.ollama_cloud",
  "claude:Cursor OAuth": "claude.cursor_oauth",
  "claude:Cursor API Key": "claude.cursor_api_key",
  "claude:Antigravity OAuth": "claude.antigravity_oauth",
  "claude:Antigravity CLI (agy)": "claude.antigravity_cli",
  "claude:GitHub Copilot": "claude.github_copilot",
  "claude:DeepSeek Official": "claude.deepseek_account",
  "claude:AWS Bedrock (AKSK)": "claude.aws_bedrock_aksk",
  "claude:AWS Bedrock (API Key)": "claude.aws_bedrock_api_key",
  "claude:OpenRouter": "claude.openrouter",
  "claude:Nvidia": "claude.nvidia",
  "claude:DeepSeek(API Key)": "claude.deepseek_api",
  "codex:OpenAI OAuth": "codex.openai_oauth",
  "codex:Grok OAuth": "codex.grok_oauth",
  "codex:Cursor API Key": "codex.cursor_api_key",
  "codex:Cursor OAuth": "codex.cursor_oauth",
  "codex:Ollama API Key": "codex.ollama_cloud",
  "codex:OpenRouter": "codex.openrouter",
  "codex:Nvidia": "codex.nvidia",
  "codex:DeepSeek(API Key)": "codex.deepseek_api",
  "gemini:Google Official": "gemini.google_oauth",
  "gemini:Antigravity OAuth": "gemini.antigravity_oauth",
  "gemini:Antigravity CLI (agy)": "gemini.antigravity_cli",
  "gemini:Grok OAuth": "gemini.grok_oauth",
  "gemini:OpenRouter": "gemini.openrouter",
});

const adapterByAppType = Object.freeze({
  "claude:claude_oauth": ["claude_oauth_bearer_compatible", "native"],
  "claude:codex_oauth": ["claude_to_codex_oauth_responses", "native"],
  "claude:grok_oauth": ["claude_to_grok_responses", "native"],
  "claude:kiro_oauth": ["claude_kiro_codewhisperer_planned", "planned"],
  "claude:ollama_cloud": ["claude_ollama_openai_chat", "native"],
  "claude:cursor_oauth": ["claude_cursor_agentservice", "native"],
  "claude:cursor_apikey": ["claude_cursor_apikey_agentservice", "native"],
  "claude:antigravity_oauth": ["claude_antigravity_gemini_native", "native"],
  "claude:agy_oauth": ["claude_antigravity_gemini_native", "native"],
  "claude:github_copilot": ["claude_copilot_skeleton", "generic_fallback"],
  "claude:deepseek_account": ["claude_deepseek_account_planned", "planned"],
  "claude:aws_bedrock": ["claude_bedrock_signature_planned", "planned"],
  "claude:openrouter": ["claude_openrouter_compatible", "native"],
  "claude:nvidia": ["claude_nvidia_openai_chat", "native"],
  "claude:deepseek_api": ["claude_deepseek_anthropic_api", "native"],
  "codex:codex_oauth": ["codex_oauth_responses", "native"],
  "codex:grok_oauth": ["codex_grok_responses", "native"],
  "codex:cursor_apikey": ["codex_cursor_apikey_agentservice", "native"],
  "codex:cursor_oauth": ["codex_cursor_agentservice", "native"],
  "codex:ollama_cloud": ["codex_ollama_openai_compatible", "native"],
  "codex:openrouter": ["codex_openrouter_compatible", "native"],
  "codex:nvidia": ["codex_openai_chat_compatible", "native"],
  "codex:deepseek_api": ["codex_openai_chat_compatible", "native"],
  "gemini:gemini_cli": ["gemini_cli_oauth_native", "native"],
  "gemini:antigravity_oauth": ["gemini_antigravity_native", "native"],
  "gemini:agy_oauth": ["gemini_antigravity_native", "native"],
  "gemini:grok_oauth": ["gemini_to_grok_responses", "native"],
  "gemini:openrouter": ["gemini_openrouter_openai_chat", "native"],
});

const protocolByAppType = Object.freeze({
  "claude:claude_oauth": "anthropic_messages",
  "claude:codex_oauth": "openai_responses",
  "claude:grok_oauth": "openai_responses",
  "claude:kiro_oauth": "kiro_codewhisperer",
  "claude:ollama_cloud": "openai_chat",
  "claude:cursor_oauth": "cursor_agentservice",
  "claude:cursor_apikey": "cursor_agentservice",
  "claude:antigravity_oauth": "gemini_native",
  "claude:agy_oauth": "gemini_native",
  "claude:github_copilot": "openai_chat",
  "claude:deepseek_account": "deepseek_chat",
  "claude:aws_bedrock": "aws_bedrock_converse",
  "claude:openrouter": "anthropic_messages",
  "claude:nvidia": "openai_chat",
  "claude:deepseek_api": "anthropic_messages",
  "codex:codex_oauth": "openai_responses",
  "codex:grok_oauth": "openai_responses",
  "codex:cursor_apikey": "cursor_agentservice",
  "codex:cursor_oauth": "cursor_agentservice",
  "codex:ollama_cloud": "openai_chat",
  "codex:openrouter": "openai_responses",
  "codex:nvidia": "openai_chat",
  "codex:deepseek_api": "openai_chat",
  "gemini:gemini_cli": "gemini_native",
  "gemini:antigravity_oauth": "gemini_native",
  "gemini:agy_oauth": "gemini_native",
  "gemini:grok_oauth": "openai_responses",
  "gemini:openrouter": "openai_chat",
});

function endpointPath(protocol) {
  return {
    anthropic_messages: "/v1/messages",
    openai_responses: "/v1/responses",
    openai_chat: "/v1/chat/completions",
    gemini_native: "/v1beta/models/{actualModel}:{method}",
    cursor_agentservice: "Cursor AgentService ConnectRPC endpoint",
    kiro_codewhisperer: "Amazon Q/CodeWhisperer generateAssistantResponse",
    deepseek_chat: "DeepSeek account chat completion endpoint",
    aws_bedrock_converse: "/model/{actualModel}/converse-stream",
  }[protocol];
}

function authContract(app, providerType, presetName) {
  const type = providerType;
  if (presetName === "AWS Bedrock (AKSK)") {
    return {
      headers: ["authorization", "x-amz-content-sha256", "x-amz-date", "x-amz-security-token?"],
      credentialSource: [
        "provider.settingsConfig.env.AWS_ACCESS_KEY_ID",
        "provider.settingsConfig.env.AWS_SECRET_ACCESS_KEY",
        "provider.settingsConfig.env.AWS_SESSION_TOKEN?",
      ],
    };
  }
  if (presetName === "AWS Bedrock (API Key)") {
    return {
      headers: ["authorization"],
      credentialSource: ["provider.settingsConfig.env.ANTHROPIC_AUTH_TOKEN"],
    };
  }
  if (type === "claude_oauth") {
    return { headers: ["authorization", "anthropic-version"], credentialSource: ["accountBinding.accountId -> AccountStore access token"] };
  }
  if (type === "codex_oauth") {
    return { headers: ["authorization", "chatgpt-account-id", "originator", "version"], credentialSource: ["accountBinding.accountId -> AccountStore access token/workspace"] };
  }
  if (type === "grok_oauth") {
    return { headers: ["authorization", "x-grok-client-identifier", "x-grok-client-version", "x-grok-conv-id", "openai-beta"], credentialSource: ["accountBinding.accountId -> AccountStore access token"] };
  }
  if (type === "kiro_oauth") {
    return { headers: ["authorization", "x-amz-user-agent", "x-amz-target", "x-amzn-kiro-agent-mode"], credentialSource: ["accountBinding.accountId -> AccountStore access token"] };
  }
  if (type === "cursor_oauth" || type === "cursor_apikey") {
    return { headers: ["authorization", "x-cursor-checksum", "x-cursor-client-version", "x-request-id"], credentialSource: [type === "cursor_oauth" ? "accountBinding.accountId -> AccountStore access token" : "provider.settingsConfig auth/API key"] };
  }
  if (["antigravity_oauth", "agy_oauth", "github_copilot", "deepseek_account"].includes(type)) {
    return { headers: ["authorization"], credentialSource: ["accountBinding.accountId -> AccountStore managed credential"] };
  }
  if (app === "gemini") {
    return { headers: ["authorization"], credentialSource: ["provider.settingsConfig.env.GEMINI_API_KEY|GOOGLE_API_KEY|API_KEY"] };
  }
  return {
    headers: ["authorization"],
    credentialSource: [
      app === "claude"
        ? "provider.settingsConfig.env.ANTHROPIC_AUTH_TOKEN|ANTHROPIC_API_KEY|API_KEY"
        : "provider.settingsConfig.auth.OPENAI_API_KEY|env.OPENAI_API_KEY|API_KEY|TOML env_key",
    ],
  };
}

function knownStatus(app, providerType, presetName) {
  if (presetName === "AWS Bedrock (API Key)") {
    return { status: "known_broken", reason: "legacy classification collapses Bearer API key and AKSK into planned SigV4 AwsBedrock" };
  }
  if (presetName === "AWS Bedrock (AKSK)") {
    return { status: "known_broken", reason: "SigV4 request planner exists but adapter capability intentionally remains planned" };
  }
  if (["kiro_oauth", "deepseek_account"].includes(providerType)) {
    return { status: "partial", reason: "production has a special forward branch while manual test/discovery still use the generic adapter" };
  }
  if (["cursor_oauth", "cursor_apikey"].includes(providerType)) {
    return { status: "partial", reason: "production uses Cursor AgentService while manual test/discovery do not execute the same operation path" };
  }
  if (providerType === "github_copilot") {
    return { status: "partial", reason: "production adds managed Copilot auth but capability remains generic_fallback" };
  }
  if (app === "gemini" && providerType !== "gemini_cli" && providerType !== "grok_oauth") {
    return { status: "partial", reason: "legacy Gemini path does not enforce the target single-model policy" };
  }
  return { status: "supported", reason: null };
}

function buildLegacyBehavior(inventory, coverage) {
  const fixtureMap = new Map();
  for (const [app, fixtures] of Object.entries(coverage.fixtures)) {
    for (const fixture of fixtures) fixtureMap.set(`${app}:${fixture.name}`, fixture);
  }
  const entries = [];
  for (const [app, presets] of Object.entries(inventory.presets)) {
    for (const preset of presets) {
      const key = `${app}:${preset.name}`;
      const fixture = fixtureMap.get(key);
      if (!fixture) throw new Error(`missing classification fixture for ${key}`);
      const providerType = fixture.expectedProviderType;
      const adapter = adapterByAppType[`${app}:${providerType}`];
      const protocol = protocolByAppType[`${app}:${providerType}`];
      const profileId = profileByPreset[key];
      if (!adapter || !protocol || !profileId) throw new Error(`unclassified behavior for ${key}`);
      const cursor = ["cursor_oauth", "cursor_apikey"].includes(providerType);
      const special =
        cursor ||
        (app === "claude" && ["kiro_oauth", "deepseek_account"].includes(providerType));
      const modelStrategy =
        (app === "claude" && providerType === "claude_oauth") ||
        (app === "codex" && providerType === "codex_oauth") ||
        (app === "gemini" && providerType === "gemini_cli")
          ? "passthrough"
          : app === "gemini" && providerType !== "grok_oauth"
            ? "legacy_requested_model"
            : "single";
      entries.push({
        legacyIdentity: { app, sourceIndex: preset.sourceIndex, presetName: preset.name },
        targetProfileId: profileId,
        classification: { providerType, evidence: "assets/contract/provider-coverage.json fixture" },
        production: {
          branch: special ? "special_forwarder" : "generic_adapter_with_provider_contract",
          adapter: adapter[0],
          adapterSupport: adapter[1],
        },
        endpoint: {
          origin: preset.baseUrl ?? "provider type default",
          sourceLocator:
            app === "claude"
              ? "provider.settingsConfig.env.ANTHROPIC_BASE_URL"
              : app === "codex"
                ? "provider.settingsConfig config.toml/base_url or env.OPENAI_BASE_URL"
                : "provider.settingsConfig.env.GOOGLE_GEMINI_BASE_URL",
          path: endpointPath(protocol),
        },
        upstreamProtocol: protocol,
        auth: authContract(app, providerType, preset.name),
        model: {
          requested: "downstream request model",
          actual:
            modelStrategy === "single"
              ? preset.defaultModel ?? "configured modelMapping.upstreamModel"
              : "downstream request model",
          strategy: modelStrategy,
        },
        capabilities: {
          stream: cursor ? "special_stream_translation" : "native_or_translated_stream",
          tools: cursor || ["kiro_oauth", "deepseek_account"].includes(providerType) ? "special_tool_bridge" : "native_or_protocol_transform",
          images:
            ["codex_oauth", "grok_oauth"].includes(providerType)
              ? "generation_route_and_request_content"
              : "request_content_only_when_protocol_supports_it",
        },
        specialSideEffects: [
          ...(providerType === "codex_oauth" ? ["OAuth refresh", "Codex identity headers", "compact request normalization"] : []),
          ...(providerType === "grok_oauth" ? ["OAuth refresh", "session identity", "model normalization", "media routing"] : []),
          ...(providerType === "github_copilot" ? ["Copilot token exchange", "request optimizer"] : []),
          ...(cursor ? ["Cursor session/tool bridge"] : []),
          ...(providerType === "kiro_oauth" ? ["Kiro token refresh", "tool name bridge"] : []),
          ...(providerType === "deepseek_account" ? ["DeepSeek account token refresh"] : []),
        ],
        operationParity: {
          manualTestUsesProductionBranch: !special && providerType !== "github_copilot",
          discoveryUsesProductionBranch: false,
        },
        knownStatus: knownStatus(app, providerType, preset.name),
      });
    }
  }
  return {
    schemaVersion: 1,
    authority: "phase-0-legacy-regression-baseline",
    serverBaselineCommit,
    sourceEvidence: pinnedSources(repoRoot, serverBaselineCommit, [
      "src/domain/providers/model.rs",
      "src/domain/providers/model_routing.rs",
      "src/proxy/adapters.rs",
      "src/proxy/forwarder.rs",
      "src/proxy/grok.rs",
      "src/proxy/kiro.rs",
      "src/proxy/deepseek.rs",
      "src/proxy/cursor/agent_driver.rs",
    ]),
    entries,
    counts: Object.fromEntries(
      ["claude", "codex", "gemini"].map((app) => [
        app,
        entries.filter((entry) => entry.legacyIdentity.app === app).length,
      ]),
    ),
  };
}

const writerSourcePaths = Object.freeze([
  "src/admin.rs",
  "src/api/invoke/dispatch.rs",
  "src/api/providers.rs",
  "src/api/types/providers.rs",
  "src/domain/providers/credentials.rs",
  "src/domain/providers/migrate.rs",
  "src/domain/providers/storage_migration.rs",
  "src/domain/providers/store.rs",
  "src/domain/providers/store_v2.rs",
  "src/infra/backup.rs",
  "src/infra/credentials.rs",
  "src/state.rs",
]);

function workingTreeSources(paths) {
  return paths.map((relativePath) => ({
    path: relativePath,
    sha256: sha256(fs.readFileSync(path.join(repoRoot, relativePath))),
  }));
}

function buildWriterInventory() {
  const writer = (
    id,
    source,
    currentBehavior,
    targetCommand,
    closePhase,
    closureStatus,
    risk,
    evidence,
  ) => ({
    id,
    source,
    currentBehavior,
    targetCommand,
    closePhase,
    closureStatus,
    risk,
    evidence,
  });
  return {
    schemaVersion: 1,
    authority: "phase-0-provider-writer-inventory",
    serverBaselineCommit,
    currentSourceEvidence: workingTreeSources(writerSourcePaths),
    entries: [
      writer(
        "rest-create",
        "src/api/providers.rs::create_provider",
        "requires the Provider write contract, applies a ProviderWriteDraft through upsert_provider_draft_command, commits clone/validate/persist/swap, and returns a redacted ProviderView",
        "retain provider_commands::create_provider over the shared commit coordinator",
        "phase-1",
        "closed",
        "a compatibility endpoint could bypass CredentialPatch or return the stored secret-bearing record",
        [
          "src/api/providers.rs::create_provider",
          "src/state.rs::upsert_provider_draft_command",
          "src/domain/providers/credentials.rs::ProviderView",
        ],
      ),
      writer(
        "rest-update",
        "src/api/providers.rs::update_provider",
        "checks path/body identity, requires the Provider write contract, and uses the same ProviderWriteDraft command and redacted response as create",
        "retain provider_commands::update_provider over the shared commit coordinator",
        "phase-1",
        "closed",
        "create and update semantics could drift or allow a resource id substitution",
        [
          "src/api/providers.rs::update_provider",
          "src/state.rs::upsert_provider_draft_command",
          "src/domain/providers/credentials.rs::CredentialPatch",
        ],
      ),
      writer(
        "rest-import",
        "src/api/providers.rs::import_providers",
        "requires explicit preview/apply mode; preview validates without writing, while apply binds the same typed batch and current snapshot to a token checked inside the commit coordinator",
        "retain provider_commands::preview_import|apply_import",
        "phase-1",
        "closed",
        "a compatibility import path could bypass preview binding, batch validation, or CredentialPatch handling",
        [
          "src/api/providers.rs::import_providers",
          "src/api/types/providers.rs::ImportProvidersRequest",
          "src/state.rs::preview_provider_import_command",
          "src/state.rs::apply_provider_import_command",
        ],
      ),
      writer(
        "rest-preset-create",
        "src/api/providers.rs::create_provider_from_preset",
        "uses profileId as the authoritative identity and writes a typed resource through the shared command; mutable preset name remains a read-only request fallback during the compatibility window",
        "retain provider_commands::create_from_profile_id and remove only the name fallback after the compatibility gate",
        "phase-10-window",
        "partial",
        "removing the deprecated name fallback before two stable releases and 14 days would break an older Web bundle",
        [
          "src/api/providers.rs::create_provider_from_preset",
          "src/api/providers.rs::profile_for_legacy_preset",
          "src/state.rs::upsert_provider_draft_command",
        ],
      ),
      writer(
        "invoke-add-update",
        "src/api/invoke/dispatch.rs::add_provider|update_provider",
        "requires the same write contract and delegates both compatibility commands to upsert_provider_draft_command with CredentialPatch",
        "retain provider_commands::create_provider|update_provider",
        "phase-1",
        "closed",
        "future invoke compatibility changes could diverge from REST validation or credential semantics",
        [
          "src/api/invoke/dispatch.rs::add_provider",
          "src/api/invoke/dispatch.rs::upsert_provider_draft_command",
        ],
      ),
      writer(
        "invoke-sort",
        "src/api/invoke/dispatch.rs::update_providers_sort_order",
        "uses the commit coordinator and stores per-app presentation order in the ProviderStore top-level order map without changing Provider revision",
        "retain provider_commands::update_provider_order over ProviderStore::update_sort_order",
        "phase-3",
        "closed",
        "a future compatibility change could reintroduce sortIndex mutation into runtime-bearing Provider records",
        [
          "src/api/invoke/dispatch.rs::update_providers_sort_order",
          "src/domain/providers/store.rs::update_sort_order",
        ],
      ),
      writer(
        "delete-cascade",
        "src/api/providers.rs::delete_provider",
        "delegates to delete_provider_command, which holds the reference mutation gate and rejects Share/current-provider references without mutating them",
        "retain provider_commands::delete_provider with reference preview and conflict response",
        "phase-3",
        "closed",
        "a future force-delete path could bypass the reference gate or revive cross-store cascade side effects",
        [
          "src/api/providers.rs::delete_provider",
          "src/state.rs::provider_reference_preview",
          "src/state.rs::delete_provider_command",
        ],
      ),
      writer(
        "save-on-load",
        "src/domain/providers/store.rs::ProviderStore::load_or_default",
        "decodes S1 without writing; legacy classification and model normalization are prepared only in the in-memory runtime view",
        "retain pure S1 decoder plus LegacyRuntimeView",
        "phase-1",
        "closed",
        "a future loader normalization could silently mutate persisted Provider behavior during startup",
        [
          "src/domain/providers/store.rs::load_or_default",
          "src/domain/providers/store.rs::prepare_legacy_runtime_view",
        ],
      ),
      writer(
        "universal-startup-migration",
        "src/domain/providers/migrate.rs::migrate_remove_universal_layer",
        "startup performs read-only inspection; mutation is available only through the explicit admin preview/apply command",
        "retain provider_migration::preview|apply",
        "phase-1",
        "closed",
        "calling the apply function from ordinary startup would restore an implicit save-on-load migration",
        [
          "src/state.rs::inspect_remove_universal_layer",
          "src/admin.rs::migrate_remove_universal_layer",
          "src/domain/providers/migrate.rs::inspect_remove_universal_layer",
        ],
      ),
      writer(
        "model-discovery-merge",
        "src/api/providers.rs::fetch_provider_models",
        "returns discovery observations without writing; merge=true is rejected and users must explicitly save a selected model through the typed Provider command",
        "retain read-only discovery operation",
        "phase-4",
        "closed",
        "a future convenience merge could couple untrusted network observations to Provider configuration and revision",
        [
          "src/api/providers.rs::fetch_provider_models",
          "src/api/providers.rs::automatic model discovery merge is retired",
        ],
      ),
      writer(
        "state-provider-mutation",
        "src/state.rs::mutate_providers_immediate*",
        "a detached commit coordinator materializes a candidate, validates and compiles RuntimePlan, seals credentials, atomically persists in the store's current S1/S2 format, reconciles post-rename errors, then swaps live state; fresh stores start as S2",
        "retain provider commit coordinator clone/validate/compile/seal/persist/swap",
        "phase-9",
        "closed",
        "new Provider writers could bypass S2 sealing or weaken the disk/live commit-point reconciliation",
        [
          "src/state.rs::commit_provider_change",
          "src/state.rs::commit_provider_change_owned",
          "src/domain/providers/store.rs::seal_for_commit",
          "src/domain/providers/store.rs::validate_for_commit",
        ],
      ),
      writer(
        "provider-store-codec",
        "src/domain/providers/store.rs::ProviderStore::save",
        "preserves S1 only for an existing unmigrated installation, writes encrypted guarded S2 for fresh or explicitly migrated stores, and never silently downgrades S2",
        "retain format-aware ProviderStore codec until the compatibility removal gate",
        "phase-9",
        "closed",
        "a raw serializer or format reset could disclose plaintext credentials or make an S2 file readable as S1",
        [
          "src/domain/providers/store.rs::ProviderStoreFormat::S2",
          "src/domain/providers/store.rs::encode_s2",
          "src/domain/providers/store_v2.rs::encode_s2",
          "src/domain/providers/store_v2.rs::old-decoder-must-reject",
        ],
      ),
      writer(
        "provider-storage-migration",
        "src/domain/providers/storage_migration.rs::apply|rollback|cleanup_snapshot",
        "runs only as an explicit offline command under the data-directory process lock, performs read-only preflight and RuntimePlan parity, snapshots S1, atomically cuts over to S2, and permits audited snapshot rollback or cleanup",
        "retain explicit provider storage migration commands",
        "phase-9",
        "closed",
        "running migration beside a live Server or deleting the rollback snapshot implicitly could corrupt the store or remove the downgrade path",
        [
          "src/domain/providers/storage_migration.rs::pub fn apply",
          "src/domain/providers/storage_migration.rs::pub fn rollback",
          "src/domain/providers/storage_migration.rs::pub fn cleanup_snapshot",
          "src/domain/providers/storage_migration.rs::apply_is_rejected_while_server_holds_data_directory_lock",
        ],
      ),
      writer(
        "backup-restore",
        "src/infra/backup.rs::restore_backup_with_validator",
        "stages all files and validates typed stores, matching Account/Provider keys, authenticated S2 credential decryption, reserved sentinels, and Provider RuntimePlan compilation before live replacement; files remain sequentially replaced by the accepted same-installation restore boundary",
        "retain same-installation staged schema/key/credential/RuntimePlan preflight",
        "phase-9",
        "closed",
        "an apply-time filesystem failure can require the generated pre-restore snapshot for rollback",
        [
          "src/infra/backup.rs::restore_backup_with_validator",
          "src/state.rs::validate_server_backup_restore_stage",
          "src/state.rs::restore_backup_command",
          "src/state.rs::s2_backup_restore_validates_key_and_runtime_before_live_replacement",
        ],
      ),
      writer(
        "reload-after-restore",
        "src/state.rs::reload_persistent_stores",
        "preloads every typed store before ordered in-memory swaps under the Provider commit lock; cross-store online transactional visibility is explicitly outside the product boundary",
        "retain validated same-installation reload boundary",
        "complete",
        "accepted_boundary",
        "concurrent readers may observe an ordered reload transition, so restore remains an explicit administrative operation",
        [
          "src/state.rs::reload_persistent_stores",
          "src/state.rs::reload_persistent_stores_under_provider_commit",
        ],
      ),
      writer(
        "ordinary-export",
        "src/api/providers.rs::export_providers",
        "returns redacted ProviderView records without reusable credentials or ciphertext",
        "retain redacted ProviderView contract",
        "complete",
        "closed",
        "a DTO regression could disclose credentials or make an ordinary export reusable as a secret bundle",
        [
          "src/api/providers.rs::export_providers",
          "src/domain/providers/credentials.rs::ProviderView",
        ],
      ),
    ],
  };
}

const compatibilitySourcePaths = Object.freeze([
  "src/api/mod.rs",
  "src/api/providers.rs",
  "src/domain/providers/credentials.rs",
  "src/domain/providers/model.rs",
  "src/domain/providers/model_routing.rs",
  "src/domain/providers/runtime.rs",
  "src/domain/providers/store.rs",
]);

function buildCompatibilityInventory() {
  const retained = (id, kind, source, marker, heuristicInputs, boundary) => ({
    id,
    kind,
    source,
    marker,
    heuristicInputs,
    boundary,
    status: "retained_until_release_window",
    removalGate: "two_stable_releases_and_14_days",
  });
  return {
    schemaVersion: 1,
    authority: "phase-10-provider-compatibility-window",
    policy: {
      decision: "retain",
      minimumStableReleases: 2,
      minimumObservationDays: 14,
      observationStartedAt: null,
      observedStableReleases: [],
      removalEligible: false,
      reason:
        "No stable bridge release observation has been recorded; deleting S1 or older-Web readers would violate the Provider migration rollback contract.",
    },
    currentSourceEvidence: workingTreeSources(compatibilitySourcePaths),
    entries: [
      retained(
        "s1-store-decoder",
        "reader",
        "src/domain/providers/store.rs",
        "pub fn load_or_default",
        [],
        "Decodes only an unguarded S1 providers.json; guarded S2 uses the strict S2 decoder.",
      ),
      retained(
        "legacy-provider-classifier",
        "reader",
        "src/domain/providers/model.rs",
        "pub fn classify_provider",
        ["name", "endpoint_url", "meta.providerType", "meta.apiFormat"],
        "Used only for S1/no-profile and legacy_compat classification; fixed and custom Profiles use canonical Registry identity.",
      ),
      retained(
        "legacy-model-routing-normalizer",
        "reader",
        "src/domain/providers/model_routing.rs",
        "normalize_provider_model_routing",
        ["name", "endpoint_url", "meta.providerType", "legacy_config"],
        "Builds the in-memory S1 compatibility runtime view and never writes during load.",
      ),
      retained(
        "legacy-runtime-driver-resolver",
        "reader",
        "src/domain/providers/runtime.rs",
        "fn legacy_driver_id",
        ["name", "meta.apiFormat", "providerType"],
        "Selected only for no-profile/legacy compatibility execution; typed Profiles dispatch by driverId.",
      ),
      retained(
        "legacy-profile-adoption-hint",
        "reader",
        "src/domain/providers/credentials.rs",
        "legacy name matching is only an adoption hint",
        ["name"],
        "Suggests an explicit reviewed adoption action and never mutates Provider identity automatically.",
      ),
      retained(
        "preset-name-create-fallback",
        "request_compatibility",
        "src/api/providers.rs",
        "profileId is required to create a Provider from a preset",
        ["name"],
        "Older Web requests may omit profileId; the fallback resolves a Registry mapping before the typed command.",
      ),
      retained(
        "provider-presets-endpoint",
        "endpoint",
        "src/api/providers.rs",
        "async fn provider_presets",
        [],
        "Deprecated shape generated from Registry identity for older Web clients; it is not a runtime authority.",
      ),
      retained(
        "provider-matrix-endpoint",
        "endpoint",
        "src/api/mod.rs",
        "async fn provider_matrix",
        [],
        "Compatibility diagnostics only; the Registry is the authoritative list of creatable Profiles.",
      ),
      retained(
        "provider-type-endpoint",
        "endpoint",
        "src/api/mod.rs",
        "async fn provider_type",
        ["name", "endpoint_url", "meta.providerType", "meta.apiFormat"],
        "Legacy classification diagnostics only; typed Provider writes cannot use it to choose identity.",
      ),
    ],
  };
}

function extractFunction(source, marker) {
  const start = source.indexOf(marker);
  if (start < 0) throw new Error(`router baseline marker missing: ${marker}`);
  const next = source.indexOf("\n    pub async fn ", start + marker.length);
  return source.slice(start, next < 0 ? source.length : next);
}

function buildRouterBaseline() {
  const models = gitShow(routerRoot, routerBaselineCommit, "src/models.rs");
  const main = gitShow(routerRoot, routerBaselineCommit, "src/main.rs");
  const store = gitShow(routerRoot, routerBaselineCommit, "src/store.rs");
  const recordSnapshot = extractFunction(store, "pub async fn record_share_runtime_snapshot");
  const facts = {
    appAvailabilityDtoDeclared:
      models.includes("pub app_availability: ShareAppAvailability") ||
      models.includes("pub app_availability: MarketAppAvailability"),
    appAvailabilityPersistedByRuntimeSnapshot: recordSnapshot.includes("snapshot.app_availability"),
    runtimeRefreshIntervalSeconds: main.includes("Duration::from_secs(600)") ? 600 : null,
    sameRevisionStaticOverwriteAllowed: store.includes(
      "WHERE excluded.config_revision >= shares.config_revision",
    ),
  };
  if (
    facts.appAvailabilityDtoDeclared !== true ||
    facts.appAvailabilityPersistedByRuntimeSnapshot !== false ||
    facts.runtimeRefreshIntervalSeconds !== 600 ||
    facts.sameRevisionStaticOverwriteAllowed !== true
  ) {
    throw new Error(`router baseline facts changed: ${JSON.stringify(facts)}`);
  }
  return {
    schemaVersion: 1,
    authority: "phase-0-router-channel-baseline",
    router: { repository: routerRoot, commit: routerBaselineCommit },
    sources: [
      { path: "src/models.rs", sha256: sha256(models) },
      { path: "src/main.rs", sha256: sha256(main) },
      { path: "src/store.rs", sha256: sha256(store) },
    ],
    facts,
    ownership: {
      staticDescriptor: [
        "identity and owner",
        "ACL and user grants",
        "limits and expiry",
        "app/provider bindings",
        "upstream provider projection",
        "config revision (later descriptor generation/fingerprint)",
      ],
      runtimeSnapshot: [
        "support status",
        "app runtimes/providers",
        "token/request counters",
        "share status",
        "model health",
        "appAvailability DTO currently ignored by persistence",
      ],
      requestLog: ["request identity", "requested/actual model", "usage", "status/latency", "caller metadata"],
    },
    followUpBoundary: "dynamic Provider health/appAvailability persistence remains outside the core Provider form plan",
  };
}

function validateWriterEvidence(writers) {
  const allowedStatuses = new Set(["open", "partial", "closed", "accepted_boundary"]);
  const evidenceByPath = new Map();
  for (const entry of writers.currentSourceEvidence ?? []) {
    if (
      typeof entry.path !== "string" ||
      !entry.path ||
      evidenceByPath.has(entry.path) ||
      !/^[0-9a-f]{64}$/.test(entry.sha256 ?? "")
    ) {
      throw new Error(`invalid Provider writer source evidence: ${entry.path ?? "<missing>"}`);
    }
    const absolutePath = path.join(repoRoot, entry.path);
    if (!fs.existsSync(absolutePath)) {
      throw new Error(`Provider writer source is missing: ${entry.path}`);
    }
    const source = fs.readFileSync(absolutePath);
    if (sha256(source) !== entry.sha256) {
      throw new Error(`stale Provider writer source evidence: ${entry.path}`);
    }
    evidenceByPath.set(entry.path, source.toString("utf8"));
  }
  for (const requiredPath of writerSourcePaths) {
    if (!evidenceByPath.has(requiredPath)) {
      throw new Error(`missing Provider writer source evidence: ${requiredPath}`);
    }
  }

  for (const entry of writers.entries) {
    if (!allowedStatuses.has(entry.closureStatus)) {
      throw new Error(`invalid Provider writer closure status: ${entry.id}`);
    }
    if (!Array.isArray(entry.evidence) || entry.evidence.length === 0) {
      throw new Error(`Provider writer lacks authoritative evidence: ${entry.id}`);
    }
    for (const locator of entry.evidence) {
      if (typeof locator !== "string") {
        throw new Error(`invalid Provider writer evidence locator: ${entry.id}`);
      }
      const separator = locator.indexOf("::");
      if (separator < 1 || separator === locator.length - 2) {
        throw new Error(`invalid Provider writer evidence locator: ${locator}`);
      }
      const relativePath = locator.slice(0, separator);
      const marker = locator.slice(separator + 2);
      const source = evidenceByPath.get(relativePath);
      if (source === undefined) {
        throw new Error(`untracked Provider writer evidence source: ${relativePath}`);
      }
      if (!source.includes(marker)) {
        throw new Error(`Provider writer evidence marker is stale: ${locator}`);
      }
    }
  }
}

function validateCompatibilityInventory(compatibility) {
  const policy = compatibility.policy ?? {};
  if (
    policy.decision !== "retain" ||
    policy.minimumStableReleases !== 2 ||
    policy.minimumObservationDays !== 14 ||
    policy.observationStartedAt !== null ||
    !Array.isArray(policy.observedStableReleases) ||
    policy.observedStableReleases.length !== 0 ||
    policy.removalEligible !== false
  ) {
    throw new Error("Provider compatibility readers cannot be removed without a completed release window");
  }
  const evidence = new Map(
    (compatibility.currentSourceEvidence ?? []).map((entry) => [entry.path, entry]),
  );
  for (const pathName of compatibilitySourcePaths) {
    const entry = evidence.get(pathName);
    const absolutePath = path.join(repoRoot, pathName);
    if (!entry || sha256(fs.readFileSync(absolutePath)) !== entry.sha256) {
      throw new Error(`stale Provider compatibility source evidence: ${pathName}`);
    }
  }
  const ids = new Set();
  for (const entry of compatibility.entries ?? []) {
    if (
      !entry.id ||
      ids.has(entry.id) ||
      entry.status !== "retained_until_release_window" ||
      entry.removalGate !== "two_stable_releases_and_14_days"
    ) {
      throw new Error(`invalid Provider compatibility entry: ${entry.id ?? "<missing>"}`);
    }
    ids.add(entry.id);
    const source = fs.readFileSync(path.join(repoRoot, entry.source), "utf8");
    if (!source.includes(entry.marker)) {
      throw new Error(`stale Provider compatibility marker: ${entry.id}`);
    }
  }
  if (JSON.stringify(compatibility) !== JSON.stringify(buildCompatibilityInventory())) {
    throw new Error("stale Provider compatibility inventory; regenerate it after review");
  }
}

export function validatePhase0Contracts(contracts) {
  const { fields, behavior, writers, compatibility, router } = contracts;
  for (const [name, contract] of Object.entries(contracts)) {
    if (contract?.schemaVersion !== 1) throw new Error(`${name} schemaVersion must be 1`);
  }
  if (behavior.entries.length !== 29 || JSON.stringify(behavior.counts) !== JSON.stringify({ claude: 16, codex: 8, gemini: 5 })) {
    throw new Error("legacy behavior contract must contain reviewed 16/8/5 presets");
  }
  const identities = new Set();
  for (const entry of behavior.entries) {
    const key = `${entry.legacyIdentity.app}:${entry.legacyIdentity.sourceIndex}:${entry.legacyIdentity.presetName}`;
    if (identities.has(key)) throw new Error(`duplicate legacy behavior identity: ${key}`);
    identities.add(key);
    for (const required of [
      entry.targetProfileId,
      entry.classification?.providerType,
      entry.production?.branch,
      entry.production?.adapter,
      entry.endpoint?.sourceLocator,
      entry.endpoint?.path,
      entry.upstreamProtocol,
      entry.model?.strategy,
      entry.capabilities?.stream,
      entry.capabilities?.tools,
      entry.capabilities?.images,
      entry.knownStatus?.status,
    ]) {
      if (typeof required !== "string" || !required) throw new Error(`incomplete legacy behavior: ${key}`);
    }
    if (!Array.isArray(entry.auth?.headers) || !Array.isArray(entry.auth?.credentialSource)) {
      throw new Error(`legacy auth evidence is missing: ${key}`);
    }
  }
  const fieldLocators = new Set();
  for (const entry of [...fields.presetFields, ...fields.persistedFields]) {
    if (!entry.locator || fieldLocators.has(entry.locator)) throw new Error(`duplicate or empty field locator: ${entry.locator}`);
    fieldLocators.add(entry.locator);
    if (!entry.classification || typeof entry.secret !== "boolean" || !entry.targetOwner || !entry.migration || !entry.retirePhase) {
      throw new Error(`unclassified Provider field: ${entry.locator}`);
    }
    if (
      (entry.secret || ["runtime", "runtime_container", "account_binding", "credential", "credential_locator"].includes(entry.classification)) &&
      (!Array.isArray(entry.reader) || entry.reader.length === 0) &&
      entry.disposition !== "blocker"
    ) {
      throw new Error(`runtime/secret field lacks reader evidence: ${entry.locator}`);
    }
    if (!Array.isArray(entry.writer) || entry.writer.length === 0) {
      throw new Error(`field lacks writer evidence: ${entry.locator}`);
    }
  }
  if (!Array.isArray(fields.dynamicPolicies) || fields.dynamicPolicies.length < 4) {
    throw new Error("dynamic and flatten field policies are incomplete");
  }
  const writerIds = new Set();
  for (const entry of writers.entries) {
    if (writerIds.has(entry.id)) throw new Error(`duplicate Provider writer: ${entry.id}`);
    writerIds.add(entry.id);
    for (const value of [entry.source, entry.currentBehavior, entry.targetCommand, entry.closePhase, entry.risk]) {
      if (typeof value !== "string" || !value) throw new Error(`incomplete Provider writer: ${entry.id}`);
    }
  }
  const requiredWriters = [
    "rest-create",
    "rest-update",
    "invoke-add-update",
    "save-on-load",
    "universal-startup-migration",
    "model-discovery-merge",
    "invoke-sort",
    "rest-import",
    "rest-preset-create",
    "ordinary-export",
    "backup-restore",
    "delete-cascade",
    "state-provider-mutation",
    "reload-after-restore",
    "provider-store-codec",
    "provider-storage-migration",
  ];
  for (const id of requiredWriters) {
    if (!writerIds.has(id)) throw new Error(`missing Provider writer inventory entry: ${id}`);
  }
  if (writerIds.size !== requiredWriters.length) {
    throw new Error(
      `Provider writer inventory must contain exactly ${requiredWriters.length} reviewed entries`,
    );
  }
  validateWriterEvidence(writers);
  validateCompatibilityInventory(compatibility);
  if (JSON.stringify(writers) !== JSON.stringify(buildWriterInventory())) {
    throw new Error("stale Provider writer inventory; regenerate it after reviewing current source");
  }
  if (
    router.facts?.appAvailabilityDtoDeclared !== true ||
    router.facts?.appAvailabilityPersistedByRuntimeSnapshot !== false ||
    router.facts?.runtimeRefreshIntervalSeconds !== 600 ||
    router.facts?.sameRevisionStaticOverwriteAllowed !== true
  ) {
    throw new Error("Router baseline facts are incomplete or inconsistent");
  }
}

function generatedContracts() {
  const inventory = readJson("assets/contract/server-provider-legacy-inventory.json");
  const coverage = readJson("assets/contract/provider-coverage.json");
  const contracts = {
    fields: buildFieldConsumption(inventory),
    behavior: buildLegacyBehavior(inventory, coverage),
    writers: buildWriterInventory(),
    compatibility: buildCompatibilityInventory(),
    router: buildRouterBaseline(),
  };
  validatePhase0Contracts(contracts);
  return contracts;
}

function atomicWrite(relativePath, content) {
  const target = path.join(repoRoot, relativePath);
  const temporary = `${target}.tmp-${process.pid}-${crypto.randomBytes(4).toString("hex")}`;
  try {
    fs.writeFileSync(temporary, content, { mode: 0o644 });
    fs.renameSync(temporary, target);
  } finally {
    if (fs.existsSync(temporary)) fs.unlinkSync(temporary);
  }
}

function main() {
  const contracts = generatedContracts();
  const serialized = Object.fromEntries(
    Object.entries(contracts).map(([name, contract]) => [name, `${JSON.stringify(contract, null, 2)}\n`]),
  );
  if (checkMode) {
    const stale = Object.entries(contractPaths).filter(([name, relativePath]) => {
      const absolutePath = path.join(repoRoot, relativePath);
      return !fs.existsSync(absolutePath) || fs.readFileSync(absolutePath, "utf8") !== serialized[name];
    });
    if (stale.length > 0) {
      throw new Error(`stale Phase 0 Provider contracts: ${stale.map(([name]) => name).join(", ")}`);
    }
    console.log(
      `provider Phase 0 contracts ok: 29 behaviors, field ledger, ${contracts.writers.entries.length} writers, ${contracts.compatibility.entries.length} retained compatibility readers, Router channel baseline`,
    );
    return;
  }
  // All contracts are generated and validated before the first destination is replaced.
  for (const [name, relativePath] of Object.entries(contractPaths)) {
    atomicWrite(relativePath, serialized[name]);
  }
  console.log("provider Phase 0 contracts written");
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
