import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  auditSourceProvenance,
  readDirectoryEntries,
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

test("a scratch directory removed mid-walk is skipped, not fatal", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "provenance-walk-"));
  const scratch = path.join(root, ".kiro-receipt-hardlink-gone");
  fs.mkdirSync(scratch);
  fs.rmSync(scratch, { recursive: true });
  try {
    assert.deepEqual(readDirectoryEntries(scratch), []);
    assert.deepEqual(auditSourceProvenance(root), []);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
