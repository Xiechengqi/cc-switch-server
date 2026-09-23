#!/usr/bin/env node
import fs from "node:fs";
import crypto from "node:crypto";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const profilePath = path.join(
  repoRoot,
  "assets/contract/claude-oauth-wire-profile.json",
);
const domainPath = path.join(repoRoot, "src/domain/claude_cli.rs");
const modelDomainPath = path.join(repoRoot, "src/domain/claude_models.rs");
const oauthPath = path.join(repoRoot, "src/proxy/claude_oauth.rs");
const thinkingPath = path.join(repoRoot, "src/proxy/thinking.rs");
const envExamplePath = path.join(repoRoot, ".env.example");
const profile = JSON.parse(fs.readFileSync(profilePath, "utf8"));
const domainSource = fs.readFileSync(domainPath, "utf8");
const modelDomainSource = fs.readFileSync(modelDomainPath, "utf8");
const oauthSource = fs.readFileSync(oauthPath, "utf8");
const thinkingSource = fs.readFileSync(thinkingPath, "utf8");
const envExample = fs.readFileSync(envExamplePath, "utf8");
const version = profile?.versions?.claudeCode;
let npmStable;

function fail(message) {
  console.error(`claude wire profile audit failed: ${message}`);
  process.exitCode = 1;
}

if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  fail("versions.claudeCode must be a three-part numeric version");
}
if (profile.schemaVersion !== 4) {
  fail("schemaVersion must be 4 for the capability-driven 2.1.280 profile");
}

const captureDate = profile?.capturedAt?.slice(0, 10);
const expectedProfileId = `claude-code-${version}-audited-${captureDate}`;
if (profile.profileId !== expectedProfileId) {
  fail(`profileId must be ${expectedProfileId}`);
}
if (Number.isNaN(Date.parse(profile.capturedAt))) {
  fail("capturedAt must be an RFC3339 timestamp");
}
if (profile.endpointIdentities?.usage !== `claude-code/${version}`) {
  fail("usage identity is not paired with versions.claudeCode");
}
if (
  profile.endpointIdentities?.bootstrap !==
    `claude-cli/${version} (external, cli)` ||
  profile.endpointIdentities?.inference?.userAgent !==
    `claude-cli/${version} (external, cli)`
) {
  fail("CLI endpoint identities are not paired with versions.claudeCode");
}
if (!domainSource.includes(`id: "${profile.profileId}"`)) {
  fail("Rust wire profile id differs from the contract asset");
}
if (!domainSource.includes(`claude_code_version: "${version}"`)) {
  fail("Rust Claude Code version differs from the contract asset");
}
for (const name of ["CC_SWITCH_CLI_UA_VERSION", "CC_SWITCH_CLI_UA"]) {
  const lines = envExample.match(new RegExp(`^${name}=.*$`, "gm")) ?? [];
  if (lines.length !== 1 || lines[0] !== `${name}=`) {
    fail(`.env.example must declare one empty ${name} override`);
  }
}
const fingerprint = profile?.billing?.promptFingerprint;
if (
  profile?.billing?.versionStrategy !==
    "public_cli_version_plus_prompt_fingerprint" ||
  profile?.billing?.billingBlockCacheControl !== false ||
  fingerprint?.algorithm !== "sha256_salted_selected_utf16_code_units" ||
  fingerprint?.salt !== "59cf53e54c78" ||
  JSON.stringify(fingerprint?.utf16CodeUnitIndices) !== "[4,7,20]" ||
  fingerprint?.missingCodeUnit !== "0" ||
  fingerprint?.digestHexCharacters !== 3 ||
  fingerprint?.promptSource !== "first_user_text_before_system_migration"
) {
  fail("billing prompt fingerprint contract is incomplete or unsupported");
}
if (
  !domainSource.includes(
    `billing_version_strategy: "${profile?.billing?.versionStrategy}"`,
  ) ||
  !domainSource.includes(
    `billing_prompt_fingerprint_salt: "${fingerprint?.salt}"`,
  )
) {
  fail("Rust billing fingerprint profile differs from the contract asset");
}
for (const vector of fingerprint?.goldenVectors ?? []) {
  const selected = fingerprint.utf16CodeUnitIndices
    .map((index) => vector.prompt[index] ?? fingerprint.missingCodeUnit)
    .join("");
  const actual = crypto
    .createHash("sha256")
    .update(`${fingerprint.salt}${selected}${vector.version}`)
    .digest("hex")
    .slice(0, fingerprint.digestHexCharacters);
  if (actual !== vector.fingerprint) {
    fail(`billing prompt fingerprint golden mismatch for ${vector.version}`);
  }
}
for (const [goldenVersion, expectedFingerprint] of [
  ["2.1.258", "1e2"],
  [version, "d7b"],
]) {
  if (
    !fingerprint?.goldenVectors?.some(
      (vector) =>
        vector.prompt === "ping" &&
        vector.version === goldenVersion &&
        vector.fingerprint === expectedFingerprint,
    )
  ) {
    fail(`billing prompt fingerprint is missing ${goldenVersion} ping golden`);
  }
}
for (const [goldenProfile, signature, billing] of [
  [
    "2.1.258-prompt-ping",
    "8d393",
    "x-anthropic-billing-header: cc_version=2.1.258.1e2; cc_entrypoint=sdk-cli; cch=00000;",
  ],
  [
    `${version}-prompt-ping`,
    "daec4",
    `x-anthropic-billing-header: cc_version=${version}.d7b; cc_entrypoint=sdk-cli; cch=00000;`,
  ],
]) {
  const cchGolden = profile?.cch?.goldenVectors?.find(
    (vector) => vector.profile === goldenProfile,
  );
  if (
    cchGolden?.signature !== signature ||
    cchGolden?.syntheticBody?.messages?.[0]?.content?.[0]?.text !== "ping" ||
    cchGolden?.syntheticBody?.system?.[0]?.text !== billing
  ) {
    fail(`CCH profile must retain the ${goldenProfile} golden`);
  }
}
const required280Betas = [
  "per-turn-control-2026-07-01",
  "timing-2026-09-09",
  "mid-conversation-tool-changes-2026-07-01",
  "inline-tools-2026-09-15",
  "mid-conversation-system-clear-at-2026-08-21",
  "dangerous-tool-use-2026-09-03",
  "thinking-binding-controls-2026-08-01",
  "thinking-resumption-2026-07-17",
  "prompt-caching-evict-2026-05-12",
];
const messageBetas = [
  ...(profile?.betaMatrices?.messages?.always ?? []),
  ...(profile?.betaMatrices?.messages?.shapeGated ?? []),
];
for (const beta of required280Betas) {
  if (!messageBetas.includes(beta) || !oauthSource.includes(beta)) {
    fail(`2.1.280 beta ${beta} is not paired across contract and runtime`);
  }
}
const opus55 = profile?.modelCatalog?.capabilitiesByModel?.["claude-opus-5-5"];
if (
  !profile?.modelCatalog?.models?.includes("claude-opus-5-5") ||
  opus55?.contextWindow !== 1_000_000 ||
  opus55?.maxOutputTokens !== 128_000 ||
  JSON.stringify(opus55?.inputModalities) !== '["text","image"]' ||
  JSON.stringify(opus55?.effortLevels) !== '["low","medium","high","xhigh","max"]' ||
  opus55?.dynamicThinking !== true ||
  opus55?.rejectsDisabledThinking !== true ||
  opus55?.midConversationSystem !== true ||
  opus55?.midConversationToolChanges !== true ||
  opus55?.perTurnEffort !== true ||
  opus55?.perTurnTiming !== true ||
  !modelDomainSource.includes('id: "claude-opus-5-5"')
) {
  fail("Opus 5.5 capability contract is incomplete or drifted from runtime");
}
if (
  !thinkingSource.includes("claude_model_capability(model)") ||
  !thinkingSource.includes('thinking_cannot_be_disabled("claude-opus-5-5")')
) {
  fail("Opus 5.5 thinking conversion is not driven by the capability registry");
}
if (
  profile?.provenance?.officialLinuxX64BinarySha256 !==
  "1e08503dbdf3c2cb0d706d32f3408277388d1c76ef108673e8fe42c1b322925b"
) {
  fail("official Claude Code 2.1.280 Linux x64 binary digest drifted");
}
if (profile.provenance?.realAccountVerification !== "pending") {
  fail("real-account evidence must stay explicitly pending until a live receipt exists");
}

if (process.argv.includes("--check-npm")) {
  const response = await fetch(
    "https://registry.npmjs.org/-/package/@anthropic-ai%2Fclaude-code/dist-tags",
    { signal: AbortSignal.timeout(10_000) },
  );
  if (!response.ok) {
    throw new Error(`npm registry returned HTTP ${response.status}`);
  }
  const tags = await response.json();
  for (const tag of ["latest", "stable"]) {
    if (!/^\d+\.\d+\.\d+$/.test(tags[tag] ?? "")) {
      throw new Error(`npm registry response has no valid ${tag} dist-tag`);
    }
  }
  npmStable = tags.stable;
  if (tags.latest !== version) {
    fail(`npm latest is ${tags.latest}, audited profile is ${version}`);
  }
}

if (!process.exitCode) {
  console.log(
    `claude wire profile audit passed (${version}${
      process.argv.includes("--check-npm")
        ? `, npm latest checked, stable observed at ${npmStable}`
        : ", offline"
    })`,
  );
}
