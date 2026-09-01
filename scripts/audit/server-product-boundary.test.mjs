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

test("test module stripping handles Rust raw strings and leaves cfg test fields intact", () => {
  const source = String.raw`
pub struct Runtime {
  #[cfg(test)]
  test_url: Option<String>,
}
#[cfg(test)]
mod tests {
  const BODY: &str = br##"{"nested":{"brace":"}"}}"##;
  /* nested comment { /* } */ } */
  fn local_http() { let _ = reqwest::Client::new(); }
}
pub fn production() {}
`;
  const stripped = stripCfgTestModules(source);
  assert.match(stripped, /test_url: Option<String>/);
  assert.match(stripped, /pub fn production/);
  assert.doesNotMatch(stripped, /reqwest::Client::new/);
});

test("only the shared HTTP transport may construct an outbound proxy", () => {
  assert.deepEqual(
    sourceBoundaryViolations(
      "src/infra/http.rs",
      "fn shared() { let _ = reqwest::Proxy::all(\"http://proxy\"); }",
    ),
    [],
  );
  assert.match(
    sourceBoundaryViolations(
      "src/example.rs",
      "fn bypass() { let _ = reqwest::Proxy::all(\"http://proxy\"); }",
    ).join("\n"),
    /bypasses the shared transport/,
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

test("runtime contract must preserve removed-feature exclusions", () => {
  const runtime = {
    excludedFeatures: [
      { id: "automaticFailover" },
      { id: "outboundProxy" },
      { id: "configTransfer" },
      { id: "usageCostAccounting" },
    ],
    commands: [],
  };
  assert.deepEqual(contractBoundaryViolations(runtime), []);
  runtime.excludedFeatures = runtime.excludedFeatures.filter(
    (feature) => feature.id !== "configTransfer",
  );
  assert.match(
    contractBoundaryViolations(runtime).join("\n"),
    /configTransfer/,
  );

  runtime.excludedFeatures.push({ id: "configTransfer" });
  runtime.commands.push({
    name: "get_model_pricing",
    feature: "usage",
    implemented: true,
  });
  assert.match(
    contractBoundaryViolations(runtime).join("\n"),
    /removed usage cost command remains registered/,
  );
});

test("Server Provider Bundle editor stays on Registry-owned boundaries", () => {
  const valid = {
    "web-src/src/ServerApp.tsx":
      'import { ProviderBundlesPage } from "@/server/providers/bundles/ProviderBundlesPage";',
    "web-src/src/server/providers/bundles/ProviderBundlesPage.tsx":
      'import { ProviderBundleEditor } from "./ProviderBundleEditor";',
    "web-src/src/server/providers/bundles/ProviderBundleEditor.tsx":
      'import { providersApi } from "@/lib/api/providers"; import { providerRegistry } from "@/server/providerRegistry";',
    "web-src/src/server/providers/editor/providerDraft.ts":
      'import { presets } from "@/server/directProviderPresets"; import { providerRegistry } from "@/server/providerRegistry";',
  };
  assert.deepEqual(providerEditorBoundaryViolations(valid), []);

  valid["web-src/src/server/providers/editor/providerDraft.ts"] =
    'import { providerRegistry } from "@/server/providerRegistry";';
  assert.match(
    providerEditorBoundaryViolations(valid).join("\n"),
    /directProviderPresets/,
  );
});

test("checked-in Server product boundary is closed", () => {
  assert.deepEqual(auditServerProductBoundary(), []);
});
