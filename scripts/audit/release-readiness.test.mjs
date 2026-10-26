import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const readinessScript = path.join(repoRoot, "scripts/release-readiness.sh");

test("release Rust regressions inherit the established 64 MiB test-thread stack", () => {
  const workflow = fs.readFileSync(
    path.join(repoRoot, ".github/workflows/_build-release-assets.yml"),
    "utf8",
  );
  const validationJob = workflow
    .split(/^  validate:\r?\n/m)[1]
    ?.split(/^  \S/m)[0];
  assert.ok(validationJob, "the release validation job must exist");
  const jobEnvironment = validationJob.match(
    /^    env:\r?\n((?:^      [^\r\n]*\r?\n)+)/m,
  )?.[1] || "";
  assert.match(jobEnvironment, /^      RUST_MIN_STACK: "67108864"$/m);

  for (const filter of ["domain::providers::registry::tests", "amazon_q", "web_session"]) {
    assert.ok(
      validationJob.includes(`          cargo test ${filter} --lib\n`),
      `the ${filter} regression must continue to run in the configured job`,
    );
  }
  for (const script of [
    "scripts/audit/validate-local.sh",
    "scripts/release-readiness.sh",
    "scripts/smoke/code-agent-regression.sh",
  ]) {
    assert.ok(
      fs.readFileSync(path.join(repoRoot, script), "utf8").includes(
        'export RUST_MIN_STACK="${RUST_MIN_STACK:-67108864}"',
      ),
      `${script} must keep the same configurable default stack budget`,
    );
  }
});

test("skipped local contracts block release readiness and evidence", () => {
  const directory = fs.mkdtempSync(
    path.join(os.tmpdir(), "cc-switch-release-readiness-"),
  );
  const evidenceFile = path.join(directory, "evidence.json");
  const environment = {
    ...process.env,
    RUN_TESTS: "0",
    RUN_REAL: "0",
    RUN_DEPLOYMENT_TESTS: "0",
    EVIDENCE_FILE: evidenceFile,
    CC_SWITCH_ROUTER_AUDIT_ROOT: path.join(directory, "missing-router"),
    CC_SWITCH_SERVER_TOKEN: "",
    ROUTER_BASE_URL: "",
    SHARE_ID: "",
    CC_SWITCH_SHARE_URL: "",
    ROUTER_API_TOKEN: "",
    CLAUDE_PROVIDER_TOKEN: "",
    CODEX_PROVIDER_TOKEN: "",
    GEMINI_PROVIDER_TOKEN: "",
    CC_SWITCH_CODEX_IMAGES_SMOKE: "0",
  };

  const result = spawnSync("bash", [readinessScript], {
    cwd: repoRoot,
    env: environment,
    encoding: "utf8",
  });

  assert.equal(result.status, 1, result.stderr);
  assert.match(
    result.stdout,
    /\[SKIP\] Router cross-repository audit unavailable while local tests are disabled/,
  );
  assert.match(result.stdout, /\[BLOCKED-INTERNAL\] local-contracts-unverified/);
  assert.match(result.stdout, /decision=blocked/);
  assert.match(result.stdout, /verificationState=blocked_inputs/);
  assert.match(result.stdout, /failures=0/);
  assert.doesNotMatch(
    result.stdout,
    /decision=ready-with-known-external-blockers/,
  );

  const evidence = JSON.parse(fs.readFileSync(evidenceFile, "utf8"));
  assert.equal(evidence.status, "blocked");
  assert.equal(evidence.verificationState, "blocked_inputs");
  assert.equal(evidence.blockerGroup, "local-contracts-unverified");
  assert.equal(evidence.checks.releaseDecision, "blocked");
  assert.equal(evidence.checks.blockedGroups, "local-contracts-unverified");
  assert.equal(evidence.checks.codexImagesGateStatus, "disabled");
  assert.match(evidence.notes, /internalBlockers=local-contracts-unverified/);
});
