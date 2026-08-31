#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const registryPath = path.join(repoRoot, "assets/contract/web-session-registry.json");
const baselinePath = path.join(repoRoot, "assets/contract/web-session-source-baseline.json");
const manifestPath = path.join(repoRoot, "assets/contract/web-session-registry-manifest.json");
const rustPath = path.join(repoRoot, "src/domain/providers/web_session.rs");
const credentialsPath = path.join(repoRoot, "src/domain/providers/credentials.rs");
const proxyPath = path.join(repoRoot, "src/proxy/web_session.rs");
const forwarderPath = path.join(repoRoot, "src/proxy/forwarder.rs");
const statePath = path.join(repoRoot, "src/state.rs");
const providersApiPath = path.join(repoRoot, "src/api/providers.rs");
const checkMode = process.argv.includes("--check");
const checkSources = process.argv.includes("--check-sources");

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function sourceIndex(baseline) {
  assert(
    baseline.format === "cc-switch-web-session-source-baseline" && baseline.schemaVersion === 1,
    "unsupported Web Session source baseline",
  );
  const sources = new Map();
  for (const source of baseline.sources) {
    assert(!sources.has(source.id), `duplicate Web Session source ${source.id}`);
    const files = new Map();
    for (const file of source.files) {
      assert(!files.has(file.path), `duplicate ${source.id} evidence file ${file.path}`);
      assert(/^[a-f0-9]{64}$/.test(file.sha256), `invalid source SHA-256 ${source.id}:${file.path}`);
      files.set(file.path, file);
    }
    sources.set(source.id, { ...source, files });
  }
  return sources;
}

function evidence(reference, sources, profileId) {
  const separator = reference.indexOf(":");
  assert(separator > 0, `${profileId} has invalid evidence reference ${reference}`);
  const sourceId = reference.slice(0, separator);
  const filePath = reference.slice(separator + 1);
  const source = sources.get(sourceId);
  assert(source, `${profileId} references unknown source ${sourceId}`);
  const file = source.files.get(filePath);
  assert(file, `${profileId} references unpinned evidence ${reference}`);
  return { sourceId, commit: source.commit, path: filePath, sha256: file.sha256 };
}

function validateProfile(profile, sources) {
  assert(profile.visibility === "hidden", `${profile.profileId} must remain hidden`);
  assert(profile.maturity === "experimental", `${profile.profileId} must remain experimental`);
  assert(
    profile.implementationState === "implemented",
    `${profile.profileId} must remain bound to the reviewed inference implementation`,
  );
  assert(profile.fixtureState === "fixture_verified", `${profile.profileId} fixture state drifted`);
  assert(profile.liveState === "live_pending", `${profile.profileId} cannot claim live evidence`);
  const endpoint = new URL(profile.path, profile.fixedOrigin);
  assert(endpoint.protocol === "https:", `${profile.profileId} must use HTTPS`);
  assert(endpoint.origin === profile.fixedOrigin, `${profile.profileId} crosses origin`);
  assert(endpoint.pathname === profile.path && !endpoint.search && !endpoint.hash, `${profile.profileId} path is not exact`);
  assert(profile.method === "POST", `${profile.profileId} method drifted`);
  assert(profile.csrfPolicy === "none_observed", `${profile.profileId} invents a CSRF flow`);
  assert(
    profile.sessionRefreshPolicy === "explicit_reimport_only",
    `${profile.profileId} adopts automatic Cookie refresh`,
  );
  assert(profile.terminal.eofWithoutTerminalIsError === true, `${profile.profileId} accepts truncated streams`);
  const expanded = profile.evidenceRefs.map((reference) => evidence(reference, sources, profile.profileId));
  assert(
    new Set(expanded.map((item) => item.sourceId)).size >= 2,
    `${profile.profileId} needs at least two independent reviewed sources`,
  );
  return {
    profileId: profile.profileId,
    providerId: profile.providerId,
    visibility: profile.visibility,
    maturity: profile.maturity,
    risk: profile.risk,
    implementationState: profile.implementationState,
    fixtureState: profile.fixtureState,
    liveState: profile.liveState,
    request: {
      fixedOrigin: profile.fixedOrigin,
      method: profile.method,
      path: profile.path,
      requestBodyLimitBytes: profile.requestBodyLimitBytes,
      responseBodyLimitBytes: profile.responseBodyLimitBytes,
    },
    credential: {
      requiredCookieFamilies: profile.requiredCookieFamilies,
      cookieRules: profile.cookieRules,
      csrfPolicy: profile.csrfPolicy,
      sessionRefreshPolicy: profile.sessionRefreshPolicy,
    },
    terminal: profile.terminal,
    evidenceFiles: expanded,
  };
}

function validateImplementationTokens() {
  const rust = fs.readFileSync(rustPath, "utf8");
  const credentials = fs.readFileSync(credentialsPath, "utf8");
  const proxy = fs.readFileSync(proxyPath, "utf8");
  const forwarder = fs.readFileSync(forwarderPath, "utf8");
  const state = fs.readFileSync(statePath, "utf8");
  const providersApi = fs.readFileSync(providersApiPath, "utf8");
  for (const token of [
    'pub const WEB_SESSION_CREDENTIAL_SLOT: &str = "/settingsConfig/webSession/cookie"',
    "WebSessionCredentialOwnership::ProviderOwned",
    "WebSessionRedirectPolicy::Disabled",
    "WebSessionCookieJarPolicy::Disabled",
    "WebSessionCrossOriginPolicy::Disabled",
    "WebSessionAuthRecovery::ExplicitReimportOnly",
    "pub fn guard_exact_request",
    "pub fn response_header_is_forwardable",
    "pub struct ParsedWebSessionCredential",
    "pub struct WebSessionScope",
    "pub struct WebSessionRuntimeStore",
    "pub fn invalidate_authentication",
    "authentication_failure_invalidates_exact_scope_without_retry_or_fallback",
    "credential_rotation_prunes_only_the_same_provider_runtime_scope",
    "provider_delete_prunes_session_task_and_invalidation_state",
  ]) {
    assert(rust.includes(token), `Web Session framework token drifted: ${token}`);
  }
  assert(
    credentials.includes('"/settingsConfig/webSession/cookie"'),
    "Web Session Provider-owned secret is not in the encrypted credential inventory",
  );
  for (const token of [
    "pub(crate) async fn execute",
    "pub(crate) fn preflight_actual_model",
    "fn build_grok_request",
    "fn build_perplexity_request",
    "fn parse_grok_ndjson",
    "fn parse_perplexity_sse",
    "read_response_body_strict",
    "explicitly re-import the reviewed session Cookie",
  ]) {
    assert(proxy.includes(token), `Web Session proxy implementation token drifted: ${token}`);
  }
  for (const token of [
    "web_session_runtime: RwLock",
    "web_session_http_client: RwLock",
    "prepare_web_session_scope",
    "invalidate_web_session_authentication",
    "record_web_session_success",
    "build_web_session_http_client",
    ".redirect(reqwest::redirect::Policy::none())",
    ".cookie_store(false)",
  ]) {
    assert(state.includes(token), `Web Session state/transport token drifted: ${token}`);
  }
  for (const token of [
    "forward_web_session(WebSessionForwardOptions",
    "web_session_auth_failure_never_retries_and_only_explicit_generation_rotation_recovers",
    "web_session_share_forward_records_estimated_usage_without_account_pooling",
  ]) {
    assert(forwarder.includes(token), `Web Session forwarder token drifted: ${token}`);
  }
  assert(
    providersApi.includes('Some("reviewed_web_session_catalog".to_string())') &&
      providersApi.includes('"liveState": "live_pending"') &&
      providersApi.includes('"entitlement": "not_asserted"'),
    "Web Session reviewed discovery contract drifted",
  );
}

function buildManifest(registry, baseline) {
  assert(
    registry.format === "cc-switch-web-session-registry" && registry.schemaVersion === 1,
    "unsupported Web Session registry",
  );
  const rail = registry.rail;
  assert(rail.id === "web_session", "Web Session rail id drifted");
  assert(rail.credentialSlot === "/settingsConfig/webSession/cookie", "Web Session secret slot drifted");
  assert(rail.credentialOwnership === "provider_owned", "Web Session secret must be Provider-owned");
  assert(rail.redirectPolicy === "disabled", "Web Session redirects must stay disabled");
  assert(rail.cookieJarPolicy === "disabled", "Web Session cookie jar must stay disabled");
  assert(rail.crossOriginPolicy === "disabled", "Web Session cross-origin traffic must stay disabled");
  assert(rail.downstreamSetCookiePolicy === "drop", "Set-Cookie must not reach downstream");
  assert(rail.authRecovery === "explicit_reimport_only", "Web Session auth cannot auto-refresh");
  assert(!rail.accountBindingAllowed, "Web Session rail cannot reuse an OAuth Account");
  assert(!rail.apiKeySlotAllowed, "Web Session rail cannot reuse an API Key slot");
  assert(!rail.extraHeadersAllowed, "Web Session rail cannot reuse extra headers");
  assert(registry.profiles.length === 2, "only two reviewed hidden candidates are expected");
  const sources = sourceIndex(baseline);
  const profiles = registry.profiles.map((profile) => validateProfile(profile, sources));
  assert(
    JSON.stringify(profiles.map((profile) => profile.profileId).sort()) ===
      JSON.stringify(["web_session.grok_web", "web_session.perplexity_web"]),
    "Web Session reviewed candidate set drifted",
  );
  validateImplementationTokens();
  return {
    format: "cc-switch-web-session-registry-manifest",
    schemaVersion: 1,
    capturedAt: baseline.capturedAt,
    generatedFrom: {
      registry: path.relative(repoRoot, registryPath),
      registrySha256: sha256(fs.readFileSync(registryPath)),
      sourceBaseline: path.relative(repoRoot, baselinePath),
      sourceBaselineSha256: sha256(fs.readFileSync(baselinePath)),
      sourceCommits: Object.fromEntries(baseline.sources.map((source) => [source.id, source.commit])),
    },
    rail,
    invariants: {
      credentialOwnership: "provider_owned",
      accountPool: false,
      crossAccountFallback: false,
      crossProviderFallback: false,
      crossCredentialRailFallback: false,
      oauthAccountReuse: false,
      apiKeySlotReuse: false,
      extraHeaderCredentialReuse: false,
      redirects: false,
      cookieJar: false,
      crossOrigin: false,
      downstreamSetCookie: false,
      authenticationRetry: false,
      explicitReimportOn401Or403: true,
      liveWithoutReceipt: false,
    },
    summary: {
      reviewedProfiles: profiles.length,
      visibleProfiles: profiles.filter((profile) => profile.visibility === "visible").length,
      inferenceImplementedProfiles: profiles.filter(
        (profile) => profile.implementationState === "implemented",
      ).length,
      fixtureState: "fixture_verified",
      liveState: "live_pending",
    },
    profiles,
    explicitlyNotAdopted: [
      "OmniRoute account pools, combo routing, cooldown selection, and connection rotation",
      "browser Cookie jars, automatic Cloudflare clearance acquisition, and redirect following",
      "Perplexity Bearer fallback and automatic Set-Cookie persistence",
      "9router rotate-cookies guidance and any cross-connection recovery",
      "unreviewed Web Cookie providers and model catalogs without independent receipts",
    ],
  };
}

function verifyExternalSources(baseline) {
  for (const source of baseline.sources) {
    const root = path.resolve(
      process.env[source.rootEnv] || path.join(repoRoot, source.defaultRelativeRoot),
    );
    assert(fs.existsSync(root), `${source.id} Web Session audit root is missing: ${root}`);
    const commit = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], {
      encoding: "utf8",
    }).trim();
    assert(commit === source.commit, `${source.id} Web Session source commit drifted`);
    for (const file of source.files) {
      const fullPath = path.join(root, file.path);
      assert(fs.existsSync(fullPath), `${source.id} evidence file is missing: ${file.path}`);
      assert(
        sha256(fs.readFileSync(fullPath)) === file.sha256,
        `${source.id} Web Session evidence drifted: ${file.path}`,
      );
    }
  }
}

const registry = readJson(registryPath);
const baseline = readJson(baselinePath);
if (checkSources) verifyExternalSources(baseline);
const manifest = `${JSON.stringify(buildManifest(registry, baseline), null, 2)}\n`;

if (checkMode) {
  assert(fs.existsSync(manifestPath), "Web Session registry manifest is missing");
  assert(
    fs.readFileSync(manifestPath, "utf8") === manifest,
    "Web Session registry manifest is stale; run scripts/audit/audit-web-session-registry.mjs",
  );
  console.log(
    `Web Session registry manifest is current${checkSources ? " and external evidence matches" : ""}`,
  );
} else {
  fs.writeFileSync(manifestPath, manifest);
  console.log("Web Session registry manifest written");
}
