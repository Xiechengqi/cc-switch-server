import assert from "node:assert/strict";
import test from "node:test";

import {
  auditSourceProvenance,
  technicalDependencyViolations,
} from "./audit-source-provenance.mjs";

test("technical dependency markers fail outside immutable history", () => {
  assert.match(
    technicalDependencyViolations(
      ".github/workflows/build.yml",
      "CC_SWITCH_PROVIDER_AUDIT_ROOT=/tmp/provider",
    ).join("\n"),
    /external Provider checkout root/,
  );
  assert.deepEqual(
    technicalDependencyViolations(
      "docs/history/retired-plan.md",
      "UPSTREAM_IMPORT.md",
    ),
    [],
  );
});

test("checked-in attribution is isolated from technical inputs", () => {
  assert.deepEqual(auditSourceProvenance(), []);
});
