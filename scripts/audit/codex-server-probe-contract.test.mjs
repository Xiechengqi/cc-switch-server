import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import { auditCodexServerProbe } from "./codex-server-probe-contract.mjs";

const asset = (name) => JSON.parse(fs.readFileSync(new URL(`../../assets/contract/${name}`, import.meta.url), "utf8"));
const report = asset("codex-r4-server-probe.json");
const codec = asset("codex-r4-compression-probe.json");

test("Codex frozen loopback measurement has complete delivery and release evidence", () => {
  auditCodexServerProbe(report, codec);
});

test("a replayed generation cannot masquerade as one measured request", () => {
  const forged = structuredClone(report);
  forged.results.find((row) => row.path === "fallback_ws").upstreamGenerations++;
  assert.throws(() => auditCodexServerProbe(forged, codec), /upstream replay/);
});

test("request completion alone cannot stand in for drained memory owners", () => {
  const forged = structuredClone(report);
  const row = forged.results.find((row) => row.path === "http_sse" && row.case === "cancel");
  const release = Object.keys(row.serverMetrics).find((key) => key.includes("phase=owner_released,"));
  delete row.serverMetrics[release];
  assert.throws(() => auditCodexServerProbe(forged, codec), /owner_released/);
});

test("measurements from different inputs or missing paths cannot become same-workload evidence", () => {
  const differentInput = structuredClone(report);
  differentInput.fixtureInputs.tool_history.sha256 = "0".repeat(64);
  assert.throws(() => auditCodexServerProbe(differentInput, codec), /same input digest/);
  const missingPath = structuredClone(report);
  missingPath.results = missingPath.results.filter((row) => row.path !== "native_ws");
  assert.throws(() => auditCodexServerProbe(missingPath, codec));
});

test("loopback/debug evidence cannot be promoted to vendor acceptance", () => {
  const promoted = structuredClone(report);
  promoted.evidenceState = "vendor_verified";
  assert.throws(() => auditCodexServerProbe(promoted, codec));
  const reordered = structuredClone(report);
  reordered.results[0].clientCompletionOrDropSeconds.p50 = Number.MAX_VALUE;
  assert.throws(() => auditCodexServerProbe(reordered, codec), /unordered percentiles/);
});
