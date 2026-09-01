#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { execFileSync } from "node:child_process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const registryPath = path.join(repoRoot, "assets/contract/provider-registry.json");
const baselinePath = path.join(
  repoRoot,
  "assets/contract/coding-plan-source-baseline.json",
);
const manifestPath = path.join(
  repoRoot,
  "assets/contract/coding-plan-registry-manifest.json",
);
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

function sorted(values) {
  return [...values].sort((left, right) => left.localeCompare(right));
}

function assertSameSet(actual, expected, label) {
  const left = sorted(actual);
  const right = sorted(expected);
  assert(
    JSON.stringify(left) === JSON.stringify(right),
    `${label} mismatch: actual=${left.join(",")} expected=${right.join(",")}`,
  );
}

function validateHttpsOrigin(value, label) {
  const url = new URL(value);
  assert(url.protocol === "https:", `${label} must use HTTPS`);
  assert(!url.username && !url.password, `${label} must not contain credentials`);
  assert(!url.search && !url.hash, `${label} must not contain query or fragment`);
  assert(url.pathname === "/", `${label} must be an origin without a path`);
  return url.origin;
}

function validateHttpsEndpoint(value, label) {
  const url = new URL(value);
  assert(url.protocol === "https:", `${label} must use HTTPS`);
  assert(!url.username && !url.password, `${label} must not contain credentials`);
  assert(!url.search && !url.hash, `${label} must not contain query or fragment`);
  assert(url.pathname.startsWith("/"), `${label} must contain an absolute path`);
  return url.toString();
}

function validateExactRoute(route, app, profileId) {
  assert(route && typeof route === "object", `${profileId} has an invalid route`);
  assert(
    typeof route.path === "string" &&
      route.path.startsWith("/") &&
      !route.path.includes("?") &&
      !route.path.includes("#") &&
      !route.path.startsWith("//"),
    `${profileId} route ${route.route} is not an exact path`,
  );
  const correctSurface =
    app === "claude"
      ? route.route === "claude_messages" || route.route === "claude_count_tokens"
      : route.route === "codex_chat_completions" || route.route === "codex_responses";
  assert(correctSurface, `${profileId} route ${route.route} crosses App surfaces`);
}

function sourceIndex(baseline) {
  const sources = new Map();
  for (const source of baseline.sources) {
    assert(!sources.has(source.id), `duplicate coding-plan source ${source.id}`);
    const files = new Map();
    for (const file of source.files) {
      assert(!files.has(file.path), `duplicate ${source.id} source file ${file.path}`);
      assert(
        /^[a-f0-9]{64}$/.test(file.sha256),
        `${source.id}:${file.path} has an invalid SHA-256`,
      );
      files.set(file.path, file);
    }
    sources.set(source.id, { ...source, files });
  }
  return sources;
}

function expandEvidenceFiles(refs, sources, label) {
  assert(Array.isArray(refs) && refs.length > 0, `${label} has no evidence files`);
  return refs.map((reference) => {
    const separator = reference.indexOf(":");
    assert(separator > 0, `${label} has invalid evidence reference ${reference}`);
    const sourceId = reference.slice(0, separator);
    const filePath = reference.slice(separator + 1);
    const source = sources.get(sourceId);
    assert(source, `${label} references unknown source ${sourceId}`);
    const file = source.files.get(filePath);
    assert(file, `${label} references unreviewed file ${reference}`);
    return {
      sourceId,
      commit: source.commit,
      path: file.path,
      sha256: file.sha256,
    };
  });
}

function validateProfile(profile, familyMetadata) {
  const profileId = profile.profileId;
  assert(
    profile.app === "claude" || profile.app === "codex",
    `${profileId} must be a Claude or Codex coding-plan surface`,
  );
  assert(profile.formComposition === "static_secret", `${profileId} must own its key`);
  assert(profile.endpointPolicy === "fixed", `${profileId} must have a fixed endpoint`);
  assert(profile.driverBinding?.kind === "fixed", `${profileId} must use a fixed driver`);
  assert(
    profile.credentialPolicy?.mode === "static_secret",
    `${profileId} cannot bind an Account credential`,
  );
  assert(profile.maturity === "experimental", `${profileId} live maturity changed without review`);

  const contract = profile.codingPlan;
  assert(contract && contract.contractRevision > 0, `${profileId} has no versioned contract`);
  const fixedOrigin = validateHttpsOrigin(
    contract.inference.fixedOrigin,
    `${profileId} inference origin`,
  );
  assert(
    contract.inference.credentialSlot.startsWith("/settingsConfig/"),
    `${profileId} inference credential must be Provider-owned`,
  );
  assert(
    profile.credentialPolicy.slots.includes(contract.inference.credentialSlot),
    `${profileId} outer credential policy does not own the inference slot`,
  );
  assert(
    contract.inference.authScheme === "api_key" ||
      contract.inference.authScheme === "bearer",
    `${profileId} has an unreviewed auth scheme`,
  );
  assert(Array.isArray(contract.routes) && contract.routes.length > 0, `${profileId} has no routes`);
  for (const route of contract.routes) validateExactRoute(route, profile.app, profileId);
  const requiredRoute = profile.app === "claude" ? "claude_messages" : "codex_responses";
  assert(
    contract.routes.some((route) => route.route === requiredRoute),
    `${profileId} is missing ${requiredRoute}`,
  );

  assert(Array.isArray(contract.models) && contract.models.length > 0, `${profileId} has no models`);
  const modelIds = new Set();
  for (const model of contract.models) {
    assert(
      typeof model.id === "string" && model.id === model.id.trim() && model.id.length > 0,
      `${profileId} has an invalid model id`,
    );
    assert(!modelIds.has(model.id), `${profileId} repeats model ${model.id}`);
    modelIds.add(model.id);
    assert(
      Number.isSafeInteger(model.contextWindow) && model.contextWindow > 0,
      `${profileId}:${model.id} has an invalid context window`,
    );
    assert(
      Array.isArray(model.inputModalities) && model.inputModalities.includes("text"),
      `${profileId}:${model.id} must explicitly support text`,
    );
    assertSameSet(
      model.inputModalities,
      new Set(model.inputModalities),
      `${profileId}:${model.id} modalities`,
    );
  }

  const quota = contract.quota;
  assert(quota && typeof quota.adapter === "string", `${profileId} has no quota contract`);
  assert(
    Number.isSafeInteger(quota.cacheTtlMs) && quota.cacheTtlMs > 0,
    `${profileId} has an invalid quota cache TTL`,
  );
  assert(
    Number.isSafeInteger(quota.staleTtlMs) && quota.staleTtlMs >= quota.cacheTtlMs,
    `${profileId} has an invalid quota stale TTL`,
  );
  if (quota.adapter === "unavailable") {
    assert(!quota.endpoint, `${profileId} unavailable quota cannot declare an endpoint`);
    assert(
      Array.isArray(quota.credentialSlots) && quota.credentialSlots.length === 0,
      `${profileId} unavailable quota cannot request credentials`,
    );
  } else {
    validateHttpsEndpoint(quota.endpoint, `${profileId} quota endpoint`);
    assert(
      Array.isArray(quota.credentialSlots) && quota.credentialSlots.length > 0,
      `${profileId} supported quota has no credential provenance`,
    );
    for (const slot of quota.credentialSlots) {
      assert(
        ["inference_credential", "access_key_id", "secret_access_key"].includes(slot.role),
        `${profileId} quota has an unknown credential role`,
      );
      assert(
        typeof slot.slot === "string" && slot.slot.startsWith("/settingsConfig/"),
        `${profileId} quota credential is not Provider-owned`,
      );
    }
  }

  const expectedTerminal =
    profile.app === "claude"
      ? ["anthropic_sse", "message_stop"]
      : contract.inference.protocol === "open_ai_responses"
        ? ["open_ai_responses_sse", "response.completed"]
        : ["open_ai_chat_sse", "[DONE]"];
  assert(
    contract.stream?.format === expectedTerminal[0] &&
      contract.stream?.terminalEvent === expectedTerminal[1] &&
      contract.stream?.errorBeforeTerminalIsFatal === true,
    `${profileId} stream terminal contract drifted`,
  );
  assert(
    contract.error?.retrySameCredentialOnceOn401 === false &&
      contract.error?.retryAfterCommit === false,
    `${profileId} introduced an unreviewed retry path`,
  );
  assert(
    contract.pricing?.evidence === "flat_rate_subscription_no_usd" &&
      typeof contract.pricing.source === "string" &&
      contract.pricing.source.trim().length > 0 &&
      Number.isFinite(Date.parse(contract.pricing.capturedAt)),
    `${profileId} is missing dated plan evidence`,
  );

  const modalities = sorted(new Set(contract.models.flatMap((model) => model.inputModalities)));
  return {
    profileId,
    app: profile.app,
    region: familyMetadata.region,
    maturity: profile.maturity,
    fixtureState: "fixture_verified",
    liveState: "live_pending",
    providerOwnedCredential: true,
    accountBindingSupported: false,
    inference: {
      fixedOrigin,
      protocol: contract.inference.protocol,
      credentialSlot: contract.inference.credentialSlot,
      authScheme: contract.inference.authScheme,
      routes: contract.routes,
    },
    catalog: {
      modelCount: contract.models.length,
      inputModalities: modalities,
      maxContextWindow: Math.max(...contract.models.map((model) => model.contextWindow)),
      tools: "not_inferred_without_explicit_model_evidence",
      models: contract.models,
    },
    quota: {
      adapter: quota.adapter,
      provenance:
        quota.adapter === "unavailable"
          ? "explicit_unavailable_no_console_cookie"
          : "reviewed_plan_api",
      endpoint: quota.endpoint ?? null,
      credentialSlots: quota.credentialSlots,
      cacheTtlMs: quota.cacheTtlMs,
      staleTtlMs: quota.staleTtlMs,
    },
    stream: contract.stream,
    error: contract.error,
    pricingEvidence: contract.pricing,
  };
}

function validateOllamaContract(registry, baseline, sources) {
  const family = registry.families.find(
    (candidate) => candidate.familyId === baseline.ollamaCloud.familyId,
  );
  assert(family, "Ollama Cloud family is missing");
  assertSameSet(
    family.surfaces.map((surface) => surface.app),
    ["claude", "codex"],
    "Ollama Cloud surfaces",
  );
  const profiles = family.surfaces.map((surface) => {
    const profile = registry.profiles.find(
      (candidate) => candidate.profileId === surface.profileId,
    );
    assert(profile, `missing Ollama Cloud profile ${surface.profileId}`);
    assert(profile.endpointPolicy === "fixed", `${profile.profileId} endpoint is not fixed`);
    assert(
      profile.formComposition === "static_secret" &&
        profile.credentialPolicy?.mode === "static_secret",
      `${profile.profileId} must use a Provider-owned API key`,
    );
    assert(profile.maturity === "stable", `${profile.profileId} maturity drifted`);
    return {
      profileId: profile.profileId,
      app: profile.app,
      maturity: profile.maturity,
      liveState: baseline.ollamaCloud.liveState,
    };
  });

  const clientSource = fs.readFileSync(
    path.join(repoRoot, "src/clients/ollama_cloud.rs"),
    "utf8",
  );
  const stateSource = fs.readFileSync(path.join(repoRoot, "src/state.rs"), "utf8");
  for (const token of [
    'const OLLAMA_CLOUD_ORIGIN: &str = "https://ollama.com"',
    'const OLLAMA_ME_PATH: &str = "/api/me"',
    'const OLLAMA_USAGE_PATH: &str = "/api/usage"',
    "const MAX_OLLAMA_RESPONSE_BYTES: usize = 512 * 1024",
    ".redirect(reqwest::redirect::Policy::none())",
    "tokio::join!",
    "Method::POST",
    "Method::GET",
  ]) {
    assert(clientSource.includes(token), `Ollama client contract token drifted: ${token}`);
  }
  for (const testName of [
    "ollama_cloud_partial_refresh_preserves_each_successful_section_as_stale",
    "ollama_authentication_failure_clears_all_cached_sections",
    "ollama_bundle_surfaces_share_one_refresh_and_delete_prunes_cached_identity",
    "ollama_refresh_discards_an_in_flight_result_after_credential_rotation",
  ]) {
    assert(stateSource.includes(testName), `missing Ollama state fixture ${testName}`);
  }

  return {
    familyId: family.familyId,
    region: baseline.ollamaCloud.region,
    credentialOwnership: "provider_owned",
    inferenceAccountRows: false,
    cookieOrHtmlCredential: false,
    profiles,
    accountProjection: {
      fixedOrigin: "https://ollama.com",
      requests: [
        { method: "POST", path: "/api/me", body: "empty" },
        { method: "GET", path: "/api/usage" },
      ],
      concurrentPartialSections: true,
      redirects: "disabled",
      maxResponseBytes: 524288,
      cacheScope: ["credential_source_key", "credential_generation"],
      staleOnlyFor: ["rate_limited", "transient"],
      authenticationFailureClearsCache: true,
      inferenceSchedulingEffect: "none_display_only",
    },
    fixtureState: "fixture_verified",
    liveState: baseline.ollamaCloud.liveState,
    evidenceFiles: expandEvidenceFiles(
      baseline.ollamaCloud.evidenceFiles,
      sources,
      "Ollama Cloud",
    ),
  };
}

function buildManifest(registry, baseline) {
  assert(
    baseline.format === "cc-switch-server-coding-plan-source-baseline" &&
      baseline.schemaVersion === 1,
    "unsupported coding-plan source baseline",
  );
  const sources = sourceIndex(baseline);
  const familyMetadata = new Map(
    baseline.families.map((family) => [family.familyId, family]),
  );
  assert(familyMetadata.size === baseline.families.length, "duplicate coding-plan family baseline");

  const profileToFamily = new Map();
  for (const family of registry.families) {
    for (const surface of family.surfaces) {
      assert(
        !profileToFamily.has(surface.profileId),
        `profile ${surface.profileId} belongs to multiple families`,
      );
      profileToFamily.set(surface.profileId, family);
    }
  }
  const codingProfiles = registry.profiles.filter((profile) => profile.codingPlan);
  const actualFamilyIds = new Set(
    codingProfiles.map((profile) => {
      const family = profileToFamily.get(profile.profileId);
      assert(family, `coding-plan profile ${profile.profileId} has no family`);
      return family.familyId;
    }),
  );
  assertSameSet(actualFamilyIds, familyMetadata.keys(), "typed coding-plan families");

  const families = baseline.families.map((metadata) => {
    const family = registry.families.find(
      (candidate) => candidate.familyId === metadata.familyId,
    );
    assert(family, `missing coding-plan family ${metadata.familyId}`);
    assertSameSet(
      family.surfaces.map((surface) => surface.app),
      ["claude", "codex"],
      `${metadata.familyId} region x Surface contract`,
    );
    const profiles = family.surfaces
      .map((surface) => {
        const profile = registry.profiles.find(
          (candidate) => candidate.profileId === surface.profileId,
        );
        assert(profile?.codingPlan, `${surface.profileId} is not a typed coding plan`);
        return validateProfile(profile, metadata);
      })
      .sort((left, right) => left.app.localeCompare(right.app));
    return {
      familyId: family.familyId,
      label: family.label,
      region: metadata.region,
      planIds: metadata.planIds,
      surfaces: profiles,
      evidenceFiles: expandEvidenceFiles(
        metadata.evidenceFiles,
        sources,
        metadata.familyId,
      ),
    };
  });

  return {
    format: "cc-switch-server-coding-plan-registry-manifest",
    schemaVersion: 1,
    capturedAt: baseline.capturedAt,
    generatedFrom: {
      providerRegistry: path.relative(repoRoot, registryPath),
      providerRegistrySha256: sha256(fs.readFileSync(registryPath)),
      sourceBaseline: path.relative(repoRoot, baselinePath),
      sourceBaselineSha256: sha256(fs.readFileSync(baselinePath)),
      sourceCommits: Object.fromEntries(
        baseline.sources.map((source) => [source.id, source.commit]),
      ),
    },
    invariants: {
      credentialOwnership: "provider_owned",
      accountPool: false,
      crossAccountFallback: false,
      crossProviderFallback: false,
      crossCredentialRailFallback: false,
      quotaSelection: false,
      consoleCookieScraping: false,
      liveWithoutReceipt: false,
    },
    summary: {
      typedFamilies: families.length,
      typedProfiles: families.reduce((total, family) => total + family.surfaces.length, 0),
      regions: sorted(new Set(families.map((family) => family.region))),
      surfaces: ["claude", "codex"],
      fixtureState: "fixture_verified",
      liveState: "live_pending",
    },
    families,
    ollamaCloud: validateOllamaContract(registry, baseline, sources),
  };
}

function verifyExternalSources(baseline) {
  for (const source of baseline.sources) {
    const root = path.resolve(
      process.env[source.rootEnv] || path.join(repoRoot, source.defaultRelativeRoot),
    );
    assert(fs.existsSync(root), `${source.id} audit root is missing: ${root}`);
    const commit = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], {
      encoding: "utf8",
    }).trim();
    assert(
      commit === source.commit,
      `${source.id} commit drift: actual=${commit} reviewed=${source.commit}`,
    );
    for (const file of source.files) {
      const fullPath = path.join(root, file.path);
      assert(fs.existsSync(fullPath), `${source.id} evidence file is missing: ${file.path}`);
      const actual = sha256(fs.readFileSync(fullPath));
      assert(
        actual === file.sha256,
        `${source.id} evidence drift: ${file.path} actual=${actual} reviewed=${file.sha256}`,
      );
    }
  }
}

const registry = readJson(registryPath);
const baseline = readJson(baselinePath);
if (checkSources) verifyExternalSources(baseline);
const manifest = `${JSON.stringify(buildManifest(registry, baseline), null, 2)}\n`;

if (checkMode) {
  assert(fs.existsSync(manifestPath), "coding-plan registry manifest is missing");
  assert(
    fs.readFileSync(manifestPath, "utf8") === manifest,
    "coding-plan registry manifest is stale; run scripts/audit/audit-coding-plan-registry.mjs",
  );
  console.log(
    `coding-plan registry manifest is current${checkSources ? " and external evidence matches" : ""}`,
  );
} else {
  fs.writeFileSync(manifestPath, manifest);
  console.log("coding-plan registry manifest written");
}
