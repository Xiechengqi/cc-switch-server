import assert from "node:assert/strict";
import test from "node:test";

import {
  auditServerProductBoundary,
  contractBoundaryViolations,
  providerEditorBoundaryViolations,
  sourceBoundaryViolations,
  stripCfgTestModules,
} from "./audit-server-product-boundary.mjs";

test("test-only reqwest clients do not weaken the production direct HTTP gate", () => {
  const source = `
pub fn production() {}
#[cfg(test)]
mod tests {
  fn local_http() { let _ = reqwest::Client::new(); }
}
`;
  assert.doesNotMatch(stripCfgTestModules(source), /reqwest::Client::new/);
  assert.deepEqual(sourceBoundaryViolations("src/example.rs", source), []);
  assert.match(
    sourceBoundaryViolations(
      "src/example.rs",
      "fn production() { let _ = reqwest::Client::builder().build(); }",
    ).join("\n"),
    /bypasses direct_client builder/,
  );
});

test("removed Provider routing and settings capabilities fail closed", () => {
  assert.match(
    sourceBoundaryViolations(
      "src/example.rs",
      "struct FailoverStore; fn configure(proxy_url: String) {}",
    ).join("\n"),
    /automatic failover/,
  );
  assert.match(
    sourceBoundaryViolations(
      "web-src/src/example.tsx",
      "const panel = <GlobalProxySettings />;",
    ).join("\n"),
    /outbound proxy configuration/,
  );
  assert.match(
    sourceBoundaryViolations(
      "web-src/src/example.tsx",
      "useImportExport();",
    ).join("\n"),
    /generic config transfer/,
  );
});

test("runtime and desktop sync contracts must preserve removed-feature exclusions", () => {
  const runtime = {
    excludedFeatures: [
      { id: "automaticFailover" },
      { id: "outboundProxy" },
      { id: "configTransfer" },
    ],
    commands: [],
  };
  const sync = {
    excluded: [
      { path: "components/settings/GlobalProxySettings.tsx" },
      { path: "components/settings/ImportExportPanel.tsx" },
    ],
  };
  assert.deepEqual(contractBoundaryViolations(runtime, sync), []);
  runtime.excludedFeatures.pop();
  assert.match(
    contractBoundaryViolations(runtime, sync).join("\n"),
    /configTransfer/,
  );
});

test("Server Provider dialogs cannot re-import the desktop dispatcher", () => {
  const valid = {
    "web-src/src/components/providers/AddProviderDialog.tsx":
      'import { ServerProviderForm } from "@/server/providers/editor/ServerProviderForm";',
    "web-src/src/components/providers/EditProviderDialog.tsx":
      'import { ServerProviderForm } from "@/server/providers/editor/ServerProviderForm";',
    "web-src/src/ServerDesktopApp.tsx":
      'import { useServerProviderActions } from "@/server/providers/useServerProviderActions";',
    "web-src/src/server/providers/useServerProviderActions.ts":
      "export function useServerProviderActions() {}",
  };
  assert.deepEqual(providerEditorBoundaryViolations(valid), []);

  valid["web-src/src/components/providers/AddProviderDialog.tsx"] =
    'import { ProviderForm } from "@/components/providers/forms/ProviderForm";';
  assert.match(
    providerEditorBoundaryViolations(valid).join("\n"),
    /desktop ProviderForm dispatcher/,
  );
});

test("checked-in Server product boundary is closed", () => {
  assert.deepEqual(auditServerProductBoundary(), []);
});
