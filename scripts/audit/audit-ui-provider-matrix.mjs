#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const checkMode = process.argv.includes("--check");

const files = {
  adapters: path.join(repoRoot, "src/proxy/adapters.rs"),
  forwarder: path.join(repoRoot, "src/proxy/forwarder.rs"),
  providerMatrix: path.join(repoRoot, "src/domain/providers/matrix.rs"),
  provider: path.join(repoRoot, "src/domain/providers/model.rs"),
  accountManagers: path.join(repoRoot, "src/domain/accounts/managers.rs"),
  accountRefresh: path.join(repoRoot, "src/clients/oauth/refresh.rs"),
  accountApi: path.join(repoRoot, "src/api/accounts.rs"),
  oauthClients: path.join(repoRoot, "src/domain/accounts/oauth.rs"),
  providerRegistry: path.join(repoRoot, "assets/contract/provider-registry.json"),
  providerRegistryModule: path.join(
    repoRoot,
    "web-src/src/server/providerRegistry.ts",
  ),
  providerBundlePage: path.join(
    repoRoot,
    "web-src/src/server/providers/bundles/ProviderBundlesPage.tsx",
  ),
  providerBundleEditor: path.join(
    repoRoot,
    "web-src/src/server/providers/bundles/ProviderBundleEditor.tsx",
  ),
  familyPicker: path.join(
    repoRoot,
    "web-src/src/server/providers/bundles/FamilyPicker.tsx",
  ),
  providerBundleCard: path.join(
    repoRoot,
    "web-src/src/server/providers/bundles/ProviderBundleCard.tsx",
  ),
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

  for (let index = start; index < input.length; index += 1) {
    const char = input[index];
    if (inString) {
      if (escaped) escaped = false;
      else if (char === "\\") escaped = true;
      else if (char === quote) inString = false;
      continue;
    }
    if (char === '"' || char === "'" || char === "`") {
      inString = true;
      quote = char;
      continue;
    }
    if (char === open) depth += 1;
    else if (char === close) {
      depth -= 1;
      if (depth === 0) return index + 1;
    }
  }
  throw new Error(`unterminated ${open}${close} literal`);
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
  const adapters = read(files.adapters);
  const forwarder = read(files.forwarder);
  const providerMatrix = read(files.providerMatrix);
  const provider = read(files.provider);
  const accountManagers = read(files.accountManagers);
  const accountRefresh = read(files.accountRefresh);
  const accountApi = read(files.accountApi);
  const oauthClients = read(files.oauthClients);
  const providerRegistry = JSON.parse(read(files.providerRegistry));
  const providerRegistryModule = read(files.providerRegistryModule);
  const providerBundlePage = read(files.providerBundlePage);
  const providerBundleEditor = read(files.providerBundleEditor);
  const familyPicker = read(files.familyPicker);
  const providerBundleCard = read(files.providerBundleCard);
  const providerMeta = read(files.providerMeta);
  const subscriptionQuery = read(files.subscriptionQuery);
  const subscriptionView = read(files.subscriptionView);

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
  const providerTypesByApp = matrixProviderTypesByApp;

  const serverTypeSet = new Set(adapterProviderTypes);
  const capabilityAppSet = new Set(capabilityApps);
  const uiTypes = new Set(Object.values(providerTypesByApp).flat());
  const registryProviderTypes = new Set();
  const errors = [];

  for (const [source, marker, label] of [
    [providerMeta, "provider.meta?.providerType === PROVIDER_TYPES.GROK_OAUTH", "managed OAuth recognition"],
    [providerMeta, 'return "grok_oauth"', "Grok quota source"],
    [providerBundleCard, 'quotaSource === "grok_oauth"', "Grok quota card dispatch"],
    [providerBundleCard, "<GrokOauthQuotaFooter", "Grok quota footer"],
    [subscriptionQuery, "useGrokOauthQuota", "Grok quota query"],
    [subscriptionQuery, 'credentialStatus !== "not_found"', "OAuth first-load refresh"],
    [subscriptionView, "grok_credits", "Grok credits tier"],
    [subscriptionView, "grok_spending_limit", "Grok spending-limit tier"],
  ]) {
    if (!source.includes(marker)) {
      errors.push(`web UI is missing ${label}`);
    }
  }

  for (const [source, marker, label] of [
    [providerRegistryModule, "provider-registry.json", "Registry contract import"],
    [familyPicker, "filterFamilies(providerRegistry.families", "Registry-backed family picker"],
    [providerBundleEditor, "<FamilyPicker", "Bundle family selection"],
    [providerBundleEditor, "createDraftForSelectedFamily", "Registry-backed bundle draft"],
    [providerBundleEditor, "toProviderBundleWriteDraft", "typed bundle writer"],
    [providerBundlePage, "<ProviderBundleEditor", "Bundle editor route"],
    [providerBundlePage, "<ProviderBundleCard", "Bundle card route"],
  ]) {
    if (!source.includes(marker)) {
      errors.push(`web UI is missing ${label}`);
    }
  }

  if (providerRegistry.format !== "cc-switch-provider-registry") {
    errors.push("provider Registry format is not cc-switch-provider-registry");
  }
  const profiles = providerRegistry.profiles ?? [];
  const families = providerRegistry.families ?? [];
  assertUnique(
    profiles.map((profile) => profile.profileId),
    "provider Registry profiles",
    errors,
  );
  assertUnique(
    families.map((family) => family.familyId),
    "provider Registry families",
    errors,
  );
  const profileById = new Map(
    profiles.map((profile) => [profile.profileId, profile]),
  );
  const familyProfileIds = new Set();
  for (const profile of profiles) {
    if (!profile.compatibilityProviderType) continue;
    registryProviderTypes.add(profile.compatibilityProviderType);
    if (!serverTypeSet.has(profile.compatibilityProviderType)) {
      errors.push(
        `Registry Profile ${profile.profileId} has unknown ProviderType ${profile.compatibilityProviderType}`,
      );
    }
    if (
      !providerTypesByApp[profile.app]?.includes(
        profile.compatibilityProviderType,
      )
    ) {
      errors.push(
        `Registry Profile ${profile.profileId} is outside the ${profile.app} UI Provider matrix`,
      );
    }
  }
  for (const family of families) {
    const surfaceProfileIds = new Set(
      family.surfaces.map((surface) => surface.profileId),
    );
    const credentialProfile = profileById.get(family.credentialProfileId);
    if (!credentialProfile) {
      errors.push(
        `Registry family ${family.familyId} has missing credential Profile ${family.credentialProfileId}`,
      );
    }
    assertUnique(
      family.surfaces.map((surface) => surface.app),
      `Registry family ${family.familyId} surfaces`,
      errors,
    );
    for (const surface of family.surfaces) {
      familyProfileIds.add(surface.profileId);
      const profile = profileById.get(surface.profileId);
      if (!profile) {
        errors.push(
          `Registry family ${family.familyId} has missing surface Profile ${surface.profileId}`,
        );
        continue;
      }
      if (profile.app !== surface.app) {
        errors.push(
          `Registry family ${family.familyId} surface ${surface.app} uses ${profile.app} Profile ${surface.profileId}`,
        );
      }
      if (
        profile.visibility !== "visible" ||
        profile.creationPolicy !== "create_allowed"
      ) {
        errors.push(
          `Registry family ${family.familyId} exposes non-creatable Profile ${surface.profileId}`,
        );
      }
    }
    if (!surfaceProfileIds.has(family.credentialProfileId)) {
      errors.push(
        `Registry family ${family.familyId} credential Profile is not a family surface`,
      );
    }
  }
  for (const profile of profiles) {
    if (
      profile.visibility === "visible" &&
      profile.creationPolicy === "create_allowed" &&
      !familyProfileIds.has(profile.profileId)
    ) {
      errors.push(
        `visible creatable Registry Profile ${profile.profileId} is unreachable from the family picker`,
      );
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
    assertUnique(types, `UI providerTypesByApp.${app}`, errors);
    for (const type of types) {
      if (!serverTypeSet.has(type)) {
        errors.push(`UI provider ${app}:${type} is not in server all_provider_types`);
      }
    }
  }

  if (!accountManagers.includes(".map(account_import_template_for)")) {
    errors.push("account_import_templates no longer maps account_provider_types");
  }
  for (const marker of [
    "pub enum AccountManagerKind",
    "pub enum AccountCredentialOwnership",
    "pub refresh_capability: OAuthRefreshCapability",
    "pub quota_capability: OAuthQuotaCapability",
    "pub inference_binding_supported: bool",
    "ProviderType::DeepSeekAccount => AccountManagerKind::ImportOnly",
  ]) {
    if (!accountManagers.includes(marker)) {
      errors.push(`account capabilities no longer expose typed ownership marker: ${marker}`);
    }
  }
  if (!accountManagers.includes("manual_token_store_with_native_refresh")) {
    errors.push("account capabilities no longer preserve the legacy manager contract");
  }
  if (!accountManagers.includes("\"manual_import_native_refresh\"")) {
    errors.push("account capabilities no longer expose manual_import_native_refresh status");
  }
  for (const marker of [
    "account_needs_native_refresh",
    "execute_native_account_refresh",
    "oauth_endpoint_fallback_allowed",
    "endpoint_fallback_safe",
    "error.is_connect()",
    "retry_after_ms",
  ]) {
    if (!accountRefresh.includes(marker)) {
      errors.push(`account refresh module no longer exposes marker: ${marker}`);
    }
  }
  for (const marker of ["build_profile_request", "refresh_account_quota"]) {
    if (!accountApi.includes(marker)) {
      errors.push(`account API no longer keeps token refresh and profile/quota enrichment separate: ${marker}`);
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
    if (!registryProviderTypes.has(type)) {
      errors.push(`account provider ${type} is not represented by a Registry Profile`);
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
    registryFamilies: families.length,
    registryProfiles: profiles.length,
    webSchema: "registry-bundle-ui",
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
  `${summary.registryFamilies} Registry families, ` +
  `${summary.registryProfiles} Registry Profiles, ` +
  `web schema ${summary.webSchema}`;

if (checkMode) {
  console.log(message);
} else {
  console.log(message);
}
