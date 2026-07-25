#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const contractPath = "assets/contract/proxy-bridge-protocol.json";
const contract = readJson(contractPath);

const requiredCases = new Map([
  ["tool-schema-root-object", "tool_schema"],
  ["tool-result-native-media", "tool_media"],
  ["reasoning-authenticated-roundtrip", "reasoning"],
  ["anthropic-request-normalization", "request_normalization"],
  ["streaming-lifecycle-ordering", "streaming_lifecycle"],
  ["semantic-failure-origin", "failure_origin"],
  ["incomplete-valid-terminal", "incomplete_terminal"],
]);

function fail(message) {
  console.error(`proxy-bridge-contract: ${message}`);
  process.exitCode = 1;
}

function readJson(relativePath) {
  try {
    return JSON.parse(fs.readFileSync(path.join(repoRoot, relativePath), "utf8"));
  } catch (error) {
    fail(`cannot read ${relativePath}: ${error.message}`);
    return {};
  }
}

if (contract.format !== "cc-switch-server-proxy-bridge-protocol") {
  fail("format must be cc-switch-server-proxy-bridge-protocol");
}
if (contract.schemaVersion !== 1) fail("schemaVersion must be 1");

const reasoning = contract.reasoningEnvelope || {};
if (reasoning.prefix !== "ccswitch-server-reasoning-v1:") {
  fail("reasoning envelope prefix drifted");
}
if (reasoning.authentication !== "hmac_sha256") {
  fail("reasoning envelopes must use HMAC-SHA256");
}
if (reasoning.maxPayloadBytes !== 2 * 1024 * 1024) {
  fail("reasoning envelope payload bound must remain 2 MiB");
}
if (reasoning.tamperPolicy !== "fail_closed") {
  fail("reasoning envelope tamper policy must fail closed");
}

const semantic = contract.semanticGuard || {};
if (semantic.enabledByDefault !== true) fail("semantic guard must default to enabled");
if (semantic.rollbackEnvironmentVariable !== "CC_SWITCH_PROXY_SEMANTIC_GUARD_ENABLED") {
  fail("semantic guard rollback environment variable drifted");
}
if (semantic.lifecycleCommitsDownstream !== false) {
  fail("lifecycle events must not commit downstream");
}
if (semantic.providerFailureFailoverBoundary !== "before_downstream_commit") {
  fail("provider semantic failover must stop at downstream commit");
}
if (semantic.clientFailurePolicy !== "forward_without_provider_penalty") {
  fail("client failures must be forwarded without provider penalty");
}
if (semantic.incompletePolicy !== "valid_partial_terminal") {
  fail("incomplete responses must remain valid partial terminals");
}

const requiredCategories = new Set(contract.requiredCategories || []);
for (const category of new Set(requiredCases.values())) {
  if (!requiredCategories.has(category)) fail(`missing required category ${category}`);
}
for (const category of requiredCategories) {
  if (![...requiredCases.values()].includes(category)) {
    fail(`unregistered required category ${category}`);
  }
}

const cases = Array.isArray(contract.cases) ? contract.cases : [];
const seen = new Set();
for (const entry of cases) {
  if (!entry.id) {
    fail("case is missing id");
    continue;
  }
  if (seen.has(entry.id)) fail(`duplicate case id ${entry.id}`);
  seen.add(entry.id);
  if (!requiredCategories.has(entry.category)) {
    fail(`case ${entry.id} has unknown category ${entry.category}`);
  }
  if (!Array.isArray(entry.assertions) || entry.assertions.length === 0) {
    fail(`case ${entry.id} must declare assertions`);
  }
  if (typeof entry.owner !== "string" || !fs.existsSync(path.join(repoRoot, entry.owner))) {
    fail(`case ${entry.id} owner does not exist: ${entry.owner}`);
  }
  if (
    typeof entry.fixture !== "string" ||
    !entry.fixture.startsWith("tests/fixtures/proxy_bridge/")
  ) {
    fail(`case ${entry.id} fixture must live under tests/fixtures/proxy_bridge`);
    continue;
  }
  const fixture = readJson(entry.fixture);
  if (fixture.id !== entry.id) fail(`fixture ${entry.fixture} id does not match ${entry.id}`);
  if (fixture.category !== entry.category) {
    fail(`fixture ${entry.fixture} category does not match ${entry.category}`);
  }
}

for (const [id, category] of requiredCases) {
  const entry = cases.find((candidate) => candidate.id === id);
  if (!entry) {
    fail(`missing required case ${id}`);
  } else if (entry.category !== category) {
    fail(`required case ${id} must use category ${category}`);
  }
}

const expectedFixtures = new Set(cases.map((entry) => entry.fixture).filter(Boolean));
const fixtureDir = path.join(repoRoot, "tests/fixtures/proxy_bridge");
for (const name of fs.existsSync(fixtureDir) ? fs.readdirSync(fixtureDir) : []) {
  if (!name.endsWith(".json")) continue;
  const relativePath = `tests/fixtures/proxy_bridge/${name}`;
  if (!expectedFixtures.has(relativePath)) fail(`unregistered proxy bridge fixture ${relativePath}`);
}

if (!process.exitCode) {
  console.log(
    `proxy-bridge-contract ok cases=${cases.length} categories=${requiredCategories.size}`,
  );
}
