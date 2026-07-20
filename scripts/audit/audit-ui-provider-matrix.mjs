#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const checkMode = process.argv.includes("--check");

const files = {
  web: path.join(repoRoot, "web-dist/index.html"),
  adapters: path.join(repoRoot, "src/proxy/adapters.rs"),
  forwarder: path.join(repoRoot, "src/proxy/forwarder.rs"),
 providerMatrix: path.join(repoRoot, "src/domain/providers/matrix.rs"),
 provider: path.join(repoRoot, "src/domain/providers/model.rs"),
 accountManagers: path.join(repoRoot, "src/domain/accounts/managers.rs"),
  accountRefresh: path.join(repoRoot, "src/clients/oauth/refresh.rs"),
  oauthClients: path.join(repoRoot, "src/domain/accounts/oauth.rs"),
  providerCard: path.join(repoRoot, "web-src/src/components/providers/ProviderCard.tsx"),
  providerMeta: path.join(repoRoot, "web-src/src/utils/providerMetaUtils.ts"),
  subscriptionQuery: path.join(repoRoot, "web-src/src/lib/query/subscription.ts"),
  subscriptionView: path.join(repoRoot, "web-src/src/components/SubscriptionQuotaFooter.tsx"),
};

function read(file) {
  return fs.readFileSync(file, "utf8");
}

function findBalanced(input, start, open, close) {
  let depth = 0;
  let inString = false;
  let quote = "";
  let escaped = false;

  for (let i = start; i < input.length; i += 1) {
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

    if (char === open) {
      depth += 1;
    } else if (char === close) {
      depth -= 1;
      if (depth === 0) return i + 1;
    }
  }
  throw new Error(`unterminated ${open}${close} literal`);
}

function extractConstObject(source, name) {
  const marker = `const ${name} =`;
  const markerIndex = source.indexOf(marker);
  if (markerIndex < 0) throw new Error(`missing ${name} in web UI`);
  const start = source.indexOf("{", markerIndex);
  if (start < 0) throw new Error(`missing object literal for ${name}`);
  const end = findBalanced(source, start, "{", "}");
  const literal = source.slice(start, end);
  return Function(`"use strict"; return (${literal});`)();
}

function extractProviderTypeMap(source) {
  const ids = new Map();
  for (const match of source.matchAll(/Self::([A-Za-z0-9]+)\s*=>\s*"([^"]+)"/g)) {
    ids.set(match[1], match[2]);
  }
  if (ids.size === 0) throw new Error("provider type as_str mapping not found");
  return ids;
}

function extractProviderArray(source, functionName, variantToId) {
  const marker = `fn ${functionName}()`;
  const markerIndex = source.indexOf(marker);
  if (markerIndex < 0) throw new Error(`missing ${functionName}`);
  const bodyStart = source.indexOf("{", markerIndex);
  if (bodyStart < 0) throw new Error(`missing function body for ${functionName}`);
  const start = source.indexOf("[", bodyStart);
  if (start < 0) throw new Error(`missing array for ${functionName}`);
  const end = findBalanced(source, start, "[", "]");
  const body = source.slice(start, end);
  const items = [...body.matchAll(/ProviderType::([A-Za-z0-9]+)/g)].map((match) => {
    const id = variantToId.get(match[1]);
    if (!id) throw new Error(`unknown ProviderType variant ${match[1]}`);
    return id;
  });
  if (items.length === 0) throw new Error(`${functionName} contains no provider types`);
  return items;
}

function extractCapabilityApps(source) {
  const match = source.match(/\[(AppKind::[^\]]+)\]\s*\n\s*\.into_iter\(\)\s*\n\s*\.flat_map/);
  if (!match) throw new Error("all_capabilities app list not found");
  return [...match[1].matchAll(/AppKind::([A-Za-z0-9]+)/g)].map((item) =>
    item[1].replace(/[A-Z]/g, (char, offset) => (offset ? "_" : "") + char.toLowerCase()),
  );
}

function appIdFromVariant(variant) {
  return variant.replace(/[A-Z]/g, (char, offset) => (offset ? "_" : "") + char.toLowerCase());
}

function extractUiProviderTypes(source, variantToId) {
  const marker = "pub fn ui_provider_types";
  const markerIndex = source.indexOf(marker);
  if (markerIndex < 0) throw new Error("missing ui_provider_types");
  const bodyStart = source.indexOf("{", markerIndex);
  const bodyEnd = findBalanced(source, bodyStart, "{", "}");
  const body = source.slice(bodyStart, bodyEnd);
  const result = {};
  for (const match of body.matchAll(/AppKind::([A-Za-z0-9]+)\s*=>\s*&\[([\s\S]*?)\]/g)) {
    const app = appIdFromVariant(match[1]);
    result[app] = [...match[2].matchAll(/ProviderType::([A-Za-z0-9]+)/g)].map((item) => {
      const id = variantToId.get(item[1]);
      if (!id) throw new Error(`unknown ProviderType variant ${item[1]}`);
      return id;
    });
  }
  if (Object.keys(result).length === 0) throw new Error("ui_provider_types has no app arms");
  return result;
}

function assertUnique(values, label, errors) {
  const seen = new Set();
  for (const value of values) {
    if (seen.has(value)) errors.push(`${label} contains duplicate ${value}`);
    seen.add(value);
  }
}

function audit() {
  const web = read(files.web);
  const adapters = read(files.adapters);
  const forwarder = read(files.forwarder);
  const providerMatrix = read(files.providerMatrix);
  const provider = read(files.provider);
  const accountManagers = read(files.accountManagers);
  const accountRefresh = read(files.accountRefresh);
  const oauthClients = read(files.oauthClients);
  const providerCard = read(files.providerCard);
  const providerMeta = read(files.providerMeta);
  const subscriptionQuery = read(files.subscriptionQuery);
  const subscriptionView = read(files.subscriptionView);

  const hasLegacyWebProviderSchema = web.includes("const fallbackProviderTypesByApp =");
  let providerTypesByApp = null;
  let providerLabels = null;
  let providerDefaults = null;
  let providerTemplateEnv = null;
  if (hasLegacyWebProviderSchema) {
    providerTypesByApp = extractConstObject(web, "fallbackProviderTypesByApp");
    providerLabels = extractConstObject(web, "fallbackProviderLabels");
    providerDefaults = extractConstObject(web, "fallbackProviderDefaults");
    providerTemplateEnv = extractConstObject(web, "fallbackProviderTemplateEnv");
  }

  const variantToId = extractProviderTypeMap(provider);
  const adapterProviderTypes = extractProviderArray(adapters, "all_provider_types", variantToId);
  const matrixProviderTypes = extractProviderArray(
    providerMatrix,
    "all_provider_types",
    variantToId,
  );
  const matrixProviderTypesByApp = extractUiProviderTypes(providerMatrix, variantToId);
  const accountProviderTypes = extractProviderArray(
    accountManagers,
    "account_provider_types",
    variantToId,
  );
  const capabilityApps = extractCapabilityApps(adapters);

  if (!hasLegacyWebProviderSchema) {
    providerTypesByApp = matrixProviderTypesByApp;
    providerLabels = Object.fromEntries(adapterProviderTypes.map((type) => [type, true]));
    providerDefaults = Object.fromEntries(adapterProviderTypes.map((type) => [type, true]));
    providerTemplateEnv = Object.fromEntries(adapterProviderTypes.map((type) => [type, true]));
  }

  const serverTypeSet = new Set(adapterProviderTypes);
  const capabilityAppSet = new Set(capabilityApps);
  const uiTypes = new Set(Object.values(providerTypesByApp).flat());
  const errors = [];

  for (const [source, marker, label] of [
    [providerMeta, "provider.meta?.providerType === PROVIDER_TYPES.GROK_OAUTH", "managed OAuth recognition"],
    [providerMeta, 'return "grok_oauth"', "Grok quota source"],
    [providerCard, 'quotaSource === "grok_oauth"', "Grok quota card dispatch"],
    [providerCard, "<GrokOauthQuotaFooter", "Grok quota footer"],
    [providerCard, "return PROVIDER_TYPES.GROK_OAUTH", "Grok account-status mapping"],
    [subscriptionQuery, "useGrokOauthQuota", "Grok quota query"],
    [subscriptionQuery, 'credentialStatus !== "not_found"', "OAuth first-load refresh"],
    [subscriptionView, "grok_credits", "Grok credits tier"],
    [subscriptionView, "grok_spending_limit", "Grok spending-limit tier"],
  ]) {
    if (!source.includes(marker)) {
      errors.push(`web UI is missing ${label}`);
    }
  }

  assertUnique(adapterProviderTypes, "adapter all_provider_types", errors);
  assertUnique(matrixProviderTypes, "provider_matrix all_provider_types", errors);
  assertUnique(accountProviderTypes, "account_provider_types", errors);

  if (adapterProviderTypes.join("\n") !== matrixProviderTypes.join("\n")) {
    errors.push("provider_matrix all_provider_types does not match adapter all_provider_types");
  }

  for (const [app, types] of Object.entries(providerTypesByApp)) {
    if (!capabilityAppSet.has(app)) {
      errors.push(`UI app ${app} has no proxy capability app`);
    }
    const matrixTypes = matrixProviderTypesByApp[app] || [];
    if (types.join("\n") !== matrixTypes.join("\n")) {
      errors.push(`fallbackProviderTypesByApp.${app} does not match provider_matrix ui_provider_types`);
    }
    assertUnique(types, `UI providerTypesByApp.${app}`, errors);
    for (const type of types) {
      if (!serverTypeSet.has(type)) {
        errors.push(`UI provider ${app}:${type} is not in server all_provider_types`);
      }
      if (!providerDefaults[type]) {
        errors.push(`UI provider ${type} is missing providerDefaults`);
      }
      if (!providerTemplateEnv[type]) {
        errors.push(`UI provider ${type} is missing providerTemplateEnv`);
      }
      if (!providerLabels[type]) {
        errors.push(`UI provider ${type} is missing providerLabels`);
      }
    }
  }

  for (const type of adapterProviderTypes) {
    if (!providerDefaults[type]) {
      errors.push(`server provider ${type} is missing UI providerDefaults`);
    }
    if (!providerTemplateEnv[type]) {
      errors.push(`server provider ${type} is missing UI providerTemplateEnv`);
    }
    if (!providerLabels[type]) {
      errors.push(`server provider ${type} is missing UI providerLabels`);
    }
  }

  for (const type of Object.keys(providerDefaults)) {
    if (!serverTypeSet.has(type)) {
      errors.push(`providerDefaults contains unknown provider ${type}`);
    }
  }
  for (const type of Object.keys(providerTemplateEnv)) {
    if (!serverTypeSet.has(type)) {
      errors.push(`providerTemplateEnv contains unknown provider ${type}`);
    }
  }
  for (const type of Object.keys(providerLabels)) {
    if (!serverTypeSet.has(type)) {
      errors.push(`providerLabels contains unknown provider ${type}`);
    }
  }

  if (!accountManagers.includes(".map(account_import_template_for)")) {
    errors.push("account_import_templates no longer maps account_provider_types");
  }
  if (!accountManagers.includes("manual_token_store_with_native_refresh")) {
    errors.push("account capabilities no longer expose manual_token_store_with_native_refresh");
  }
  if (!accountManagers.includes("\"manual_import_native_refresh\"")) {
    errors.push("account capabilities no longer expose manual_import_native_refresh status");
  }
  if (hasLegacyWebProviderSchema) {
    for (const marker of [
      "manual refresh-token import",
      "native refresh/profile",
      "refreshToken is required",
      "accountRefreshStateText",
    ]) {
      if (!web.includes(marker)) {
        errors.push(`web UI no longer renders account refresh-ready marker: ${marker}`);
      }
    }
  }
  for (const marker of [
    "account_needs_native_refresh",
    "execute_native_account_refresh",
    "profile refresh warning",
  ]) {
    if (!accountRefresh.includes(marker)) {
      errors.push(`account refresh module no longer exposes marker: ${marker}`);
    }
  }
  if (!forwarder.includes("managed account refresh failed")) {
    errors.push("proxy forwarder no longer reports managed account refresh failures");
  }
  for (const snippet of [
    "ProviderType::CodexOAuth => Some(OAuthProviderSpec {\n            provider_type,\n            stage: OAuthSupportStage::NativeRefreshProfile",
    "ProviderType::ClaudeOAuth => Some(OAuthProviderSpec {\n            provider_type,\n            stage: OAuthSupportStage::NativeRefreshProfile",
    "ProviderType::GeminiCli => Some(OAuthProviderSpec {\n            provider_type,\n            stage: OAuthSupportStage::NativeRefreshProfile",
    "ProviderType::CursorOAuth => Some(OAuthProviderSpec {\n            provider_type,\n            stage: OAuthSupportStage::NativeRefreshProfile",
    "ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => Some(OAuthProviderSpec {\n            provider_type,\n            stage: OAuthSupportStage::NativeRefreshProfile",
  ]) {
    if (!oauthClients.includes(snippet)) {
      errors.push("OAuth native refresh/profile provider marker is missing or moved");
      break;
    }
  }
  for (const type of accountProviderTypes) {
    if (!serverTypeSet.has(type)) {
      errors.push(`account provider ${type} is not in server all_provider_types`);
    }
    if (!providerDefaults[type] || !providerTemplateEnv[type]) {
      errors.push(`account provider ${type} is not importable from the Web provider schema`);
    }
  }

  if (errors.length > 0) {
    throw new Error(errors.join("\n"));
  }

  return {
    apps: capabilityApps.length,
    serverProviderTypes: adapterProviderTypes.length,
    uiProviderTypes: uiTypes.size,
    uiProviderPairs: Object.values(providerTypesByApp).reduce(
      (total, types) => total + types.length,
      0,
    ),
    diagnosticProviderPairs:
      capabilityApps.length * adapterProviderTypes.length -
      Object.values(providerTypesByApp).reduce((total, types) => total + types.length, 0),
    accountProviderTypes: accountProviderTypes.length,
    webSchema: hasLegacyWebProviderSchema ? "legacy-web-dist" : "react-web-src-pending",
  };
}

const summary = audit();
const message =
  `ui provider matrix ok: ${summary.apps} apps, ` +
  `${summary.serverProviderTypes} server provider types, ` +
  `${summary.uiProviderTypes} UI provider types, ` +
  `${summary.uiProviderPairs} UI app/provider pairs, ` +
  `${summary.diagnosticProviderPairs} diagnostic-only pairs, ` +
  `${summary.accountProviderTypes} account provider types, ` +
  `web schema ${summary.webSchema}`;

if (checkMode) {
  console.log(message);
} else {
  console.log(message);
}
