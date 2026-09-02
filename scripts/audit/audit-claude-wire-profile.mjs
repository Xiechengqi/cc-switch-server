#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const profilePath = path.join(
  repoRoot,
  "assets/contract/claude-oauth-wire-profile.json",
);
const domainPath = path.join(repoRoot, "src/domain/claude_cli.rs");
const envExamplePath = path.join(repoRoot, ".env.example");
const profile = JSON.parse(fs.readFileSync(profilePath, "utf8"));
const domainSource = fs.readFileSync(domainPath, "utf8");
const envExample = fs.readFileSync(envExamplePath, "utf8");
const version = profile?.versions?.claudeCode;

function fail(message) {
  console.error(`claude wire profile audit failed: ${message}`);
  process.exitCode = 1;
}

if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  fail("versions.claudeCode must be a three-part numeric version");
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
if (!envExample.includes(`CC_SWITCH_CLI_UA_VERSION=${version}`)) {
  fail(".env.example version override differs from the contract asset");
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
  if (!/^\d+\.\d+\.\d+$/.test(tags.stable ?? "")) {
    throw new Error("npm registry response has no valid stable dist-tag");
  }
  if (tags.stable !== version) {
    fail(`npm stable is ${tags.stable}, audited profile is ${version}`);
  }
}

if (!process.exitCode) {
  console.log(
    `claude wire profile audit passed (${version}${
      process.argv.includes("--check-npm") ? ", npm stable checked" : ", offline"
    })`,
  );
}
