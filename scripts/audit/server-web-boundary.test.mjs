import assert from "node:assert/strict";
import test from "node:test";

import {
  auditServerWebBoundary,
  extractModuleSpecifiers,
  sourceContentViolations,
} from "./audit-server-web-boundary.mjs";

test("module graph parser covers static, bare, export, and lazy imports", () => {
  assert.deepEqual(
    extractModuleSpecifiers(`
      import value from "./value";
      import "./style.css";
      export { other } from "@/other";
      const lazy = import("./lazy");
    `),
    ["./lazy", "./style.css", "./value", "@/other"],
  );
});

test("Codex Referral remains allowed while static promotion metadata fails", () => {
  assert.deepEqual(
    sourceContentViolations(
      "web-src/src/components/providers/forms/CodexReferralPanel.tsx",
      "export function CodexReferralPanel() { return 'referral tracking'; }",
    ),
    [],
  );
  assert.match(
    sourceContentViolations(
      "web-src/src/example.ts",
      'const partnerPromotionKey = "STATIC";',
    ).join("\n"),
    /static promotion metadata/,
  );
  assert.match(
    sourceContentViolations(
      "web-src/src/example.ts",
      'const url = "https://example.test/?utm_source=campaign";',
    ).join("\n"),
    /affiliate or campaign URL/,
  );
});

test("checked-in Server Web production graph is closed", () => {
  assert.deepEqual(auditServerWebBoundary(), []);
});
