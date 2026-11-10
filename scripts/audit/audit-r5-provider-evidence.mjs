#!/usr/bin/env node
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const reviewed = new Map([
  ["shared_core", { file: "r5-shared-core.json", digest: "28784e02c009539aef37d795afe9266019de2789ed0de33ea9f777b2063563b3" }],
  ["web_session", { file: "r5-web-sessions.json", digest: "899a0361ed4098073a111c9b6bb692e5cbd4f72a8622ee6f45e3f55886f7af2a" }],
  ["static_profiles", { file: "r5-static-profiles.json", digest: "f59bf17743333f6aa0373d64d46957335c413918b656beb252231241b9416377" }],
  ["deepseek_account", { file: "r5-deepseek.json", digest: "d08a5450f0db6fa71c26bff3df1d7ea4644b9f4c7ae311a0eef83018b284c047" }],
  ["trae", { file: "r5-trae.json", digest: "e49e8d1774c5d8430643db52386b928d3fc12cc9aa560898db8f25fb9a4ea9b9" }],
  ["buddy", { file: "r5-buddy.json", digest: "9e1f11a63803a5a9aba0ade07ffd5f2137891c16752f8387e2cdf342ccf243df" }],
  ["grok_oauth", { file: "r5-grok.json", digest: "581f94554b1cb022c677d4c3d6bcd2c6475a25ff7010146c7d3e24661230756f" }],
  ["cursor", { file: "r5-cursor.json", digest: "5580842e3f5a6b778efad9e4a1a4202a2d0e9897ca4d5de77edbc67d38228d31" }],
  ["codex_oauth", { file: "r5-codex-oauth.json", digest: "7a3dca76e2d6d41a39b325bcc88f432c5e3ab5c4d64a15aa579c38fae056653d" }],
  ["claude_oauth", { file: "r5-claude-oauth.json", digest: "4ed2f135deda06c891f655ca66662ddba48cc91c6cb5890904220115a2fed0b4" }],
  ["antigravity_agy", { file: "r5-antigravity.json", digest: "9e17c97b52a597541ee0e5ad65bd92dcc0657a0840dab534f5683ec9ac74cf2c" }],
  ["ollama_cloud", { file: "r5-ollama-cloud.json", digest: "ce8fb36718693f1ed5241ae46a72ab9b6c15419592dadbc74c79d5e621747804" }],
  ["kimi_code", { file: "r5-kimi-code.json", digest: "545a5aecc860705b6c5392d3829896e0de1bae489f72b3653c6bb281864fdf0f" }],
  ["gemini_cli", { file: "r5-gemini-cli.json", digest: "c717a2ff873bde0894c4fe47f5b8fdb33c41594bffa2cf93cd0fe7744808076a" }],
]);
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") return Object.fromEntries(
    Object.keys(value).sort().map((key) => [key, canonical(value[key])]),
  );
  return value;
}
export function staticProfileProjection(registry, runtimeSource) {
  const defaultBody = runtimeSource.match(/fn default_base_url\([\s\S]*?\n}\n/);
  assert(defaultBody, "runtime default origin function moved; review static origins");
  const origins = Object.fromEntries([
    ["aws_bedrock", "AwsBedrock"], ["nvidia", "Nvidia"], ["deepseek_api", "DeepSeekApi"],
  ].map(([kind, rust]) => {
    const match = defaultBody[0].match(new RegExp(`ProviderType::${rust} => Some\\("([^"\\n]+)"\\)`));
    assert(match, `missing reviewed runtime origin ${kind}`);
    return [kind, match[1]];
  }));
  const profiles = registry.profiles.filter((profile) => profile.codingPlan ||
    Object.hasOwn(origins, profile.compatibilityProviderType)).map((profile) => ({
    profileId: profile.profileId,
    app: profile.app,
    providerType: profile.compatibilityProviderType,
    endpointPolicy: profile.endpointPolicy,
    originTemplate: profile.codingPlan?.inference.fixedOrigin ?? origins[profile.compatibilityProviderType],
    driverBinding: profile.driverBinding,
    credentialPolicy: profile.credentialPolicy,
    modelPolicy: profile.modelPolicy,
    allowedModelPolicies: profile.allowedModelPolicies ?? [profile.modelPolicy],
    defaultUpstreamModel: profile.defaultUpstreamModel ?? null,
    codingPlan: profile.codingPlan ?? null,
  })).sort((a,b) => a.profileId.localeCompare(b.profileId));
  assert.equal(profiles.length, 26, "static Profile scope changed; review matrix");
  assert.equal(profiles.filter((profile) => profile.codingPlan).length, 20);
  const ids = new Set(profiles.map((profile) => profile.profileId));
  const driverIds = new Set(profiles.map((profile) => profile.driverBinding.driverId));
  const drivers = registry.drivers.filter((driver) => driverIds.has(driver.driverId)).sort((a,b) => a.driverId.localeCompare(b.driverId));
  const families = registry.families.filter((family) => family.surfaces.some((surface) => ids.has(surface.profileId))).sort((a,b) => a.familyId.localeCompare(b.familyId));
  return { profiles, drivers, families };
}

export function auditStaticProfileProjection(document, registry, runtimeSource) {
  registry ??= JSON.parse(fs.readFileSync(path.join(root,"assets/contract/provider-registry.json"),"utf8"));
  runtimeSource ??= fs.readFileSync(path.join(root,"src/domain/providers/runtime.rs"),"utf8");
  const projection = staticProfileProjection(registry, runtimeSource);
  for (const key of ["profiles","drivers","families"]) {
    assert.deepEqual(document[key], projection[key], `R5 static ${key} differ from current Registry/runtime`);
  }
  return { profiles:26, codingPlans:20 };
}

export function loadR5Evidence(providerClass) {
  assert(reviewed.has(providerClass), "unreviewed R5 provider class");
  const bytes = fs.readFileSync(path.join(root, "assets/contract", reviewed.get(providerClass).file));
  assert(bytes.length < 128 * 1024, "R5 evidence exceeds local byte cap");
  return JSON.parse(bytes.toString("utf8"));
}

export function auditSharedCoreProjection(document, input = {}) {
  const registry = input.registry ?? JSON.parse(fs.readFileSync(path.join(root, "assets/contract/provider-registry.json"), "utf8"));
  const label = (kind, driver) => ({
    "special.grok_web_session": "grok_web_session",
    "special.perplexity_web_session": "perplexity_web_session",
  })[driver] ?? kind;
  const profiles = registry.profiles.filter((profile) => profile.compatibilityProviderType).map((profile) => ({
    profileId: profile.profileId, app: profile.app,
    providerType: profile.compatibilityProviderType, driverBinding: profile.driverBinding,
    timingLabel: label(profile.compatibilityProviderType, profile.driverBinding.driverId),
  })).sort((a,b) => a.profileId.localeCompare(b.profileId));
  const customRecipes = registry.customRecipes.map((recipe) => ({
    recipeId: recipe.recipeId, profileId: recipe.profileId,
    providerType: recipe.compatibilityProviderType, binding: recipe.binding,
    timingLabel: recipe.compatibilityProviderType,
  })).sort((a,b) => a.recipeId.localeCompare(b.recipeId));
  const all = [...profiles, ...customRecipes];
  assert.deepEqual(document.scope.registry, {
    profiles, customRecipes,
    providerTypes: [...new Set(all.map((item) => item.providerType))].sort(),
    labels: [...new Set(all.map((item) => item.timingLabel))].sort(),
  }, "R5 timing Registry scope changed; review driver labels");
  const constants = (key, file, names) => {
    const source = input[key] ?? fs.readFileSync(path.join(root,file), "utf8");
    return Object.fromEntries(names.map((name) => {
      const expression = source.match(new RegExp(`const ${name}\\s*:\\s*\\w+\\s*=\\s*([0-9_\\s*]+);`))?.[1];
      assert(expression, `review resource constant ${name}`);
      const value = expression.replaceAll("_", "").split("*").reduce((value, term) => value * Number(term.trim()),1);
      assert(Number.isSafeInteger(value) && value >= 0, `invalid resource constant ${name}`);
      return [name,value];
    }));
  };
  assert.deepEqual(document.scope.resources, {
    deepseek: constants("deepseek", "src/proxy/deepseek_runtime.rs", ["MAX_SESSION_SCOPES","MAX_ACTIVE_SESSION_FLIGHTS","DEEPSEEK_SESSION_TTL_MS"]),
    trae: constants("trae", "src/proxy/trae_runtime.rs", ["MAX_RUNTIME_ENTRIES","MAX_ACTIVE_CATALOG_FLIGHTS","MAX_RUNTIME_BYTES","TRAE_MODEL_CATALOG_TTL_MS","MAX_STALE_CATALOG_AGE_MS"]),
    web: constants("web", "src/proxy/web_session_runtime.rs", ["MAX_WEB_SCOPES","MAX_SESSION_ID_BYTES"]),
  }, "R5 resource bounds changed; review ownership and evidence");
  return { profiles: profiles.length, providerTypes: 28, timingLabels: 30 };
}
export function auditR5Evidence(document) {
  assert.equal(document.schemaVersion, 1);
  assert.equal(document.kind, "synthetic_server_contract");
  assert.equal(document.liveState, "live_pending");
  const expected = reviewed.get(document.providerClass);
  assert(expected, "unreviewed R5 provider class");
  const digest = crypto.createHash("sha256").update(JSON.stringify(canonical(document))).digest("hex");
  assert.equal(digest, expected.digest, "R5 evidence, scope or local policy drifted from reviewed digest");
  if (document.providerClass === "static_profiles") auditStaticProfileProjection(document);
  if (document.providerClass === "shared_core") auditSharedCoreProjection(document);
  return { providerClass: document.providerClass, cases: document.cases.length, liveState: document.liveState };
}
export function auditAllR5Evidence() {
  return [...reviewed.keys()].map((key) => auditR5Evidence(loadR5Evidence(key)));
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log("R5 provider evidence ok:", auditAllR5Evidence());
}
