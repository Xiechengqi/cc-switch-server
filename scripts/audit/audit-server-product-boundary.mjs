#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);

const requiredExcludedFeatures = Object.freeze([
  "automaticFailover",
  "outboundProxy",
  "configTransfer",
  "usageCostAccounting",
]);

const forbiddenRuntimeCommands = Object.freeze([
  "check_provider_limits",
  "delete_model_pricing",
  "get_default_cost_multiplier",
  "get_model_pricing",
  "get_pricing_model_source",
  "set_default_cost_multiplier",
  "set_pricing_model_source",
  "update_model_pricing",
]);

const forbiddenPaths = Object.freeze([
  "assets/contract/desktop-ui-sync.json",
  "docs/server-desktop-ui-parity-plan.md",
  "scripts/sync/convert-legacy-export.mjs",
  "scripts/sync/export-current-cc-switch-fixtures.mjs",
  "scripts/sync/import-desktop-export.mjs",
  "scripts/sync/sync-desktop-ui.mjs",
  "scripts/sync/sync-desktop-ui.test.mjs",
  "src/domain/failover.rs",
  "src/domain/usage/pricing.rs",
  "src/domain/settings/transfer.rs",
  "web-src/src/components/providers/FailoverPriorityBadge.tsx",
  "web-src/src/components/proxy/AutoFailoverConfigPanel.tsx",
  "web-src/src/components/proxy/CircuitBreakerConfigPanel.tsx",
  "web-src/src/components/proxy/FailoverQueueManager.tsx",
  "web-src/src/components/proxy/FailoverToggle.tsx",
  "web-src/src/components/settings/GlobalProxySettings.tsx",
  "web-src/src/components/settings/ImportExportSection.tsx",
  "web-src/src/components/usage/ModelsDevPickerDialog.tsx",
  "web-src/src/components/usage/PricingConfigPanel.tsx",
  "web-src/src/components/usage/PricingEditModal.tsx",
  "web-src/src/desktop-theme.css",
  "web-src/src/ServerDesktopApp.tsx",
  "web-src/src/hooks/useGlobalProxy.ts",
  "web-src/src/hooks/useImportExport.ts",
  "web-src/src/hooks/useProxyConfig.ts",
  "web-src/src/lib/api/failover.ts",
  "web-src/src/lib/api/globalProxy.ts",
  "web-src/src/lib/query/failover.ts",
]);

const rustRemovedFeaturePatterns = Object.freeze([
  [
    "automatic failover",
    /\b(?:FailoverStore|FailoverQueue|AutoFailover|CircuitBreaker(?:Config|State)?)\b|\b(?:auto_failover|failover_store|circuit_breaker)\b/,
  ],
  [
    "outbound proxy configuration",
    /\b(?:GlobalProxyConfig|OutboundProxy|SystemProxy)\b|\b(?:global_proxy|outbound_proxy|system_proxy|proxy_url)\b/,
  ],
  [
    "generic config transfer",
    /\b(?:ConfigTransfer|ImportConfig|ExportConfig)\b|\b(?:import_config|export_config|config_transfer)\b/,
  ],
]);

const webRemovedFeaturePatterns = Object.freeze([
  [
    "automatic failover",
    /\b(?:FailoverPriorityBadge|AutoFailoverConfigPanel|CircuitBreakerConfigPanel|FailoverQueueManager|FailoverToggle|useFailover)\b/,
  ],
  [
    "outbound proxy configuration",
    /\b(?:GlobalProxySettings|useGlobalProxy|globalProxyApi|proxyUrl|httpProxy|httpsProxy|allProxy|systemProxy)\b/,
  ],
  [
    "generic config transfer",
    /\b(?:ImportExportSection|ImportExportPanel|useImportExport|importConfig|exportConfig|configTransfer)\b/,
  ],
]);

function walkFiles(root, extensions) {
  if (!fs.existsSync(root)) return [];
  const files = [];
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const absolutePath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== "node_modules" && entry.name !== "dist")
          stack.push(absolutePath);
      } else if (extensions.has(path.extname(entry.name))) {
        files.push(absolutePath);
      }
    }
  }
  return files.sort();
}

function matchingBrace(source, openingBrace) {
  let depth = 0;
  let quote = null;
  let escaped = false;
  let lineComment = false;
  let blockComment = false;
  for (let index = openingBrace; index < source.length; index += 1) {
    const char = source[index];
    const next = source[index + 1];
    if (lineComment) {
      if (char === "\n") lineComment = false;
      continue;
    }
    if (blockComment) {
      if (char === "*" && next === "/") {
        blockComment = false;
        index += 1;
      }
      continue;
    }
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === quote) {
        quote = null;
      }
      continue;
    }
    if (char === "/" && next === "/") {
      lineComment = true;
      index += 1;
      continue;
    }
    if (char === "/" && next === "*") {
      blockComment = true;
      index += 1;
      continue;
    }
    if (char === '"') {
      quote = char;
      continue;
    }
    if (char === "'") {
      const character = source
        .slice(index)
        .match(/^'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\])'/)?.[0];
      if (character) index += character.length - 1;
      continue;
    }
    if (char === "{") depth += 1;
    if (char === "}") {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  throw new Error("unbalanced Rust block while excluding #[cfg(test)] source");
}

export function stripCfgTestModules(source) {
  let result = source;
  const marker = "#[cfg(test)]";
  while (true) {
    const start = result.indexOf(marker);
    if (start < 0) return result;
    const openingBrace = result.indexOf("{", start + marker.length);
    if (openingBrace < 0)
      throw new Error("#[cfg(test)] block has no opening brace");
    const end = matchingBrace(result, openingBrace);
    result = `${result.slice(0, start)}\n${result.slice(end + 1)}`;
  }
}

export function sourceBoundaryViolations(relativePath, source) {
  const violations = [];
  const extension = path.extname(relativePath);
  if (extension === ".rs") {
    const production = stripCfgTestModules(source);
    for (const [label, pattern] of rustRemovedFeaturePatterns) {
      if (pattern.test(production))
        violations.push(`${relativePath}: ${label} symbol`);
    }
    if (relativePath !== "src/infra/http.rs") {
      if (/reqwest::Client::(?:new|builder)\s*\(/.test(production)) {
        violations.push(
          `${relativePath}: production reqwest client bypasses direct_client builder`,
        );
      }
    }
    if (/reqwest::Proxy\b|\.proxy\s*\(/.test(production)) {
      violations.push(
        `${relativePath}: production outbound proxy construction`,
      );
    }
  }
  if ([".ts", ".tsx", ".js", ".jsx"].includes(extension)) {
    for (const [label, pattern] of webRemovedFeaturePatterns) {
      if (pattern.test(source))
        violations.push(`${relativePath}: ${label} symbol`);
    }
  }
  return violations;
}

export function contractBoundaryViolations(runtimeContract) {
  const violations = [];
  const excluded = new Set(
    (runtimeContract.excludedFeatures ?? []).map((feature) => feature.id),
  );
  for (const feature of requiredExcludedFeatures) {
    if (!excluded.has(feature)) {
      violations.push(`web runtime contract must exclude ${feature}`);
    }
  }
  for (const command of runtimeContract.commands ?? []) {
    if (
      requiredExcludedFeatures.includes(command.feature) &&
      command.implemented
    ) {
      violations.push(
        `removed feature command remains implemented: ${command.name}`,
      );
    }
    if (forbiddenRuntimeCommands.includes(command.name)) {
      violations.push(`removed usage cost command remains registered: ${command.name}`);
    }
  }

  return violations;
}

export function providerEditorBoundaryViolations(sources) {
  const violations = [];
  for (const pathName of [
    "web-src/src/components/providers/AddProviderDialog.tsx",
    "web-src/src/components/providers/EditProviderDialog.tsx",
  ]) {
    const source = sources[pathName] ?? "";
    if (!source.includes("@/server/providers/editor/ServerProviderForm")) {
      violations.push(
        `${pathName}: core dialog must import ServerProviderForm directly`,
      );
    }
    if (source.includes("@/components/providers/forms/ProviderForm")) {
      violations.push(
        `${pathName}: core dialog imports the non-Server ProviderForm dispatcher`,
      );
    }
  }

  const app = sources["web-src/src/ServerApp.tsx"] ?? "";
  if (!app.includes("@/server/providers/useServerProviderActions")) {
    violations.push(
      "web-src/src/ServerApp.tsx: missing Server-only Provider action boundary",
    );
  }
  if (app.includes("@/hooks/useProviderActions")) {
    violations.push(
      "web-src/src/ServerApp.tsx: imports non-Server Provider action dispatcher",
    );
  }

  const actions =
    sources["web-src/src/server/providers/useServerProviderActions.ts"] ?? "";
  for (const forbidden of [
    "openclaw",
    "opencode",
    "hermes",
    "claude-desktop",
    "updateTrayMenu",
  ]) {
    if (actions.toLowerCase().includes(forbidden.toLowerCase())) {
      violations.push(
        `web-src/src/server/providers/useServerProviderActions.ts: non-core Provider action ${forbidden}`,
      );
    }
  }
  return violations;
}

export function auditServerProductBoundary(root = repoRoot) {
  const violations = [];
  for (const relativePath of forbiddenPaths) {
    if (fs.existsSync(path.join(root, relativePath))) {
      violations.push(`${relativePath}: removed feature file exists`);
    }
  }
  for (const absolutePath of [
    ...walkFiles(path.join(root, "src"), new Set([".rs"])),
    ...walkFiles(
      path.join(root, "web-src", "src"),
      new Set([".ts", ".tsx", ".js", ".jsx"]),
    ),
  ]) {
    const relativePath = path
      .relative(root, absolutePath)
      .replaceAll(path.sep, "/");
    violations.push(
      ...sourceBoundaryViolations(
        relativePath,
        fs.readFileSync(absolutePath, "utf8"),
      ),
    );
  }

  const directHttp = fs.readFileSync(
    path.join(root, "src/infra/http.rs"),
    "utf8",
  );
  if (!/reqwest::Client::builder\(\)\s*\.no_proxy\(\)/.test(directHttp)) {
    violations.push(
      "src/infra/http.rs: direct builder must explicitly disable proxies",
    );
  }
  const runtimeContract = JSON.parse(
    fs.readFileSync(
      path.join(root, "assets/contract/web-runtime-contract.json"),
      "utf8",
    ),
  );
  const runtimeDescriptions = [
    runtimeContract.product,
    ...[
      ...(runtimeContract.retainedFeatures ?? []),
      ...(runtimeContract.hiddenFeatures ?? []),
      ...(runtimeContract.excludedFeatures ?? []),
    ].flatMap((feature) => [feature.label, feature.reason]),
    ...(runtimeContract.commands ?? []).map((command) => command.notes),
  ]
    .filter(Boolean)
    .join("\n")
    .toLowerCase();
  if (runtimeDescriptions.includes("desktop")) {
    violations.push("web runtime contract may describe only Server-native commands");
  }
  violations.push(...contractBoundaryViolations(runtimeContract));
  const providerEditorPaths = [
    "web-src/src/components/providers/AddProviderDialog.tsx",
    "web-src/src/components/providers/EditProviderDialog.tsx",
    "web-src/src/ServerApp.tsx",
    "web-src/src/server/providers/useServerProviderActions.ts",
  ];
  violations.push(
    ...providerEditorBoundaryViolations(
      Object.fromEntries(
        providerEditorPaths.map((relativePath) => [
          relativePath,
          fs.readFileSync(path.join(root, relativePath), "utf8"),
        ]),
      ),
    ),
  );
  return violations;
}

function main() {
  const violations = auditServerProductBoundary();
  if (violations.length > 0) {
    throw new Error(
      `Server product boundary violations:\n${violations.join("\n")}`,
    );
  }
  console.log(
    "server product boundary ok: server-native UI, deterministic routing, direct HTTP, scoped data transfer",
  );
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main();
}
