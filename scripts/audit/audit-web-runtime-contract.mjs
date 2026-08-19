#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const contractPath = "assets/contract/web-runtime-contract.json";
const contract = JSON.parse(fs.readFileSync(contractPath, "utf8"));

function fail(message) {
  console.error(`web-runtime-contract: ${message}`);
  process.exitCode = 1;
}

function uniqueBy(items, key, label) {
  const seen = new Set();
  for (const item of items) {
    const value = item[key];
    if (!value) fail(`${label} entry missing ${key}`);
    if (seen.has(value)) fail(`duplicate ${label} ${value}`);
    seen.add(value);
  }
  return seen;
}

if (contract.uiAutomationAllowed !== false) {
  fail("uiAutomationAllowed must be false");
}

const transport = contract.clientWebTransport || {};
if (transport.privatePrefix !== "/web-api/") {
  fail("clientWebTransport.privatePrefix must be /web-api/");
}
if (transport.authentication !== "authorization_header") {
  fail("client web transport must use Authorization headers");
}
if (transport.queryTokensAllowed !== false) {
  fail("client web transport must reject query-string tokens");
}
for (const requiredPath of [
  "/web-api/events",
  "/web-api/admin/upgrade/stream",
]) {
  if (!(transport.streamPaths || []).includes(requiredPath)) {
    fail(`client web stream contract is missing ${requiredPath}`);
  }
}

const retained = contract.retainedFeatures || [];
const hidden = contract.hiddenFeatures || [];
const excluded = contract.excludedFeatures || [];
const featureIds = new Set([
  ...uniqueBy(retained, "id", "retained feature"),
  ...uniqueBy(hidden, "id", "hidden feature"),
  ...uniqueBy(excluded, "id", "excluded feature"),
]);
if (featureIds.size !== retained.length + hidden.length + excluded.length) {
  fail("feature ids must be unique across retained/hidden/excluded groups");
}

const commands = contract.commands || [];
uniqueBy(commands, "name", "command");
const commandByName = new Map(commands.map((command) => [command.name, command]));
for (const command of commands) {
  if (!["native", "shim", "excluded"].includes(command.support)) {
    fail(`command ${command.name} has invalid support ${command.support}`);
  }
  if (!featureIds.has(command.feature)) {
    fail(`command ${command.name} references unknown feature ${command.feature}`);
  }
  if (command.support === "excluded" && command.implemented) {
    fail(`excluded command ${command.name} cannot be implemented`);
  }
}

const restEndpoints = contract.restEndpoints || [];
const endpointIds = new Set();
for (const endpoint of restEndpoints) {
  const id = `${endpoint.method || ""} ${endpoint.path || ""}`;
  if (!endpoint.method || !endpoint.path) {
    fail("REST endpoint entry must include method and path");
    continue;
  }
  if (endpointIds.has(id)) fail(`duplicate REST endpoint ${id}`);
  endpointIds.add(id);
  if (!featureIds.has(endpoint.feature)) {
    fail(`REST endpoint ${id} references unknown feature ${endpoint.feature}`);
  }
}

const usageEndpointPaths = [
  "/web-api/usage/overview",
  "/web-api/usage/trends",
  "/web-api/usage/facets",
  "/web-api/usage/provider-bundles",
  "/web-api/usage/models",
  "/web-api/usage/shares",
  "/web-api/usage/requests",
  "/web-api/usage/requests/:id",
];
for (const endpointPath of usageEndpointPaths) {
  const endpoint = restEndpoints.find(
    (candidate) => candidate.method === "GET" && candidate.path === endpointPath,
  );
  if (!endpoint) {
    fail(`REST contract is missing GET ${endpointPath}`);
    continue;
  }
  if (endpoint.feature !== "usage" || endpoint.authentication !== "session") {
    fail(`GET ${endpointPath} must be a session-authenticated usage endpoint`);
  }
  if (!["data_meta", "data_meta_cursor"].includes(endpoint.responseEnvelope)) {
    fail(`GET ${endpointPath} has invalid response envelope`);
  }
}

const apiRouterSource = fs.readFileSync("src/api/mod.rs", "utf8");
for (const endpointPath of usageEndpointPaths) {
  if (!apiRouterSource.includes(`\"${endpointPath}\"`)) {
    fail(`src/api/mod.rs is missing contracted endpoint ${endpointPath}`);
  }
}

const usageEdits = contract.shareUserUsageEdits;
if (!usageEdits) {
  fail("contract must declare shareUserUsageEdits");
} else {
  if (usageEdits.ownership !== "server") {
    fail("shareUserUsageEdits.ownership must be server");
  }
  if (usageEdits.clientSuppliedGrantFieldIgnored !== true) {
    fail(
      "shareUserUsageEdits must declare that a client-supplied usageRebase is ignored",
    );
  }
  if (usageEdits.appliedUnderQuotaLock !== true) {
    fail("shareUserUsageEdits must be applied under the Share quota lock");
  }
  if (usageEdits.windowBounds !== contract.shareUserTokenPeriods?.windowBounds) {
    fail("shareUserUsageEdits.windowBounds must match shareUserTokenPeriods");
  }
  for (const commandName of usageEdits.commands || []) {
    const command = commandByName.get(commandName);
    if (!command || !command.implemented) {
      fail(
        `shareUserUsageEdits references unimplemented command ${commandName}`,
      );
    }
  }

  const contractSource = fs.readFileSync(
    "src/domain/sharing/router_contract.rs",
    "utf8",
  );
  const grantFieldPattern = new RegExp(
    `pub usage_rebase: Option<ShareUserUsageRebase>`,
  );
  if (!grantFieldPattern.test(contractSource)) {
    fail(
      `src/domain/sharing/router_contract.rs is missing the ${usageEdits.grantField} grant field`,
    );
  }
  if (usageEdits.operatorField) {
    const snake = usageEdits.operatorField.replace(
      /[A-Z]/g,
      (c) => `_${c.toLowerCase()}`,
    );
    if (!contractSource.includes(`pub ${snake}: Option<String>`)) {
      fail(
        `ShareUserUsageRebase is missing the contracted operator field ${usageEdits.operatorField}`,
      );
    }
  }
  for (const field of usageEdits.setFields || []) {
    const snake = field.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
    if (!contractSource.includes(`pub ${snake}:`)) {
      fail(`ShareUserUsageEdit is missing contracted field ${field}`);
    }
  }

  const errorSource = fs.readFileSync("src/api/error.rs", "utf8");
  for (const code of usageEdits.conflictCodes || []) {
    if (!errorSource.includes(`"${code}"`)) {
      fail(`src/api/error.rs does not emit contracted conflict code ${code}`);
    }
  }

  const handlerSource = fs.readFileSync("src/api/invoke/handlers.rs", "utf8");
  if (!handlerSource.includes(`"${usageEdits.field}"`)) {
    fail(
      `src/api/invoke/handlers.rs does not parse contracted field ${usageEdits.field}`,
    );
  }
}

const shareUsageEdit = contract.shareTotalUsageEdit;
if (!shareUsageEdit) {
  fail("contract must declare shareTotalUsageEdit");
} else {
  if (shareUsageEdit.ownership !== "server") {
    fail("shareTotalUsageEdit.ownership must be server");
  }
  if (shareUsageEdit.clientSuppliedShareFieldIgnored !== true) {
    fail(
      "shareTotalUsageEdit must declare that a client-supplied tokensUsed is ignored",
    );
  }
  if (shareUsageEdit.appliedUnderQuotaLock !== true) {
    fail("shareTotalUsageEdit must be applied under the Share quota lock");
  }
  if (shareUsageEdit.rebuiltFromHistory !== false) {
    fail(
      "shareTotalUsageEdit must declare that the Share total counter is not rebuilt from Usage history",
    );
  }
  for (const commandName of shareUsageEdit.commands || []) {
    const command = commandByName.get(commandName);
    if (!command || !command.implemented) {
      fail(
        `shareTotalUsageEdit references unimplemented command ${commandName}`,
      );
    }
  }

  const contractSource = fs.readFileSync(
    "src/domain/sharing/router_contract.rs",
    "utf8",
  );
  if (!contractSource.includes("pub struct ShareTotalUsageEdit")) {
    fail(
      "src/domain/sharing/router_contract.rs is missing ShareTotalUsageEdit",
    );
  }
  for (const field of shareUsageEdit.setFields || []) {
    const snake = field.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
    if (!contractSource.includes(`pub ${snake}:`)) {
      fail(`ShareTotalUsageEdit is missing contracted field ${field}`);
    }
  }

  const shareSource = fs.readFileSync("src/domain/sharing/shares.rs", "utf8");
  if (!shareSource.includes("fn apply_share_total_usage_edit(")) {
    fail(
      "src/domain/sharing/shares.rs must apply the Share total usage edit in the domain layer",
    );
  }

  const handlerSource = fs.readFileSync("src/api/invoke/handlers.rs", "utf8");
  if (!handlerSource.includes(`"${shareUsageEdit.field}"`)) {
    fail(
      `src/api/invoke/handlers.rs does not parse contracted field ${shareUsageEdit.field}`,
    );
  }
}

const quotaView = contract.shareUserQuotaView;
if (!quotaView) {
  fail("contract must declare shareUserQuotaView");
} else {
  if (quotaView.ownership !== "server") {
    fail("shareUserQuotaView.ownership must be server");
  }
  if (quotaView.clientSuppliedFieldIgnored !== true) {
    fail(
      "shareUserQuotaView must declare that a client-supplied usageQuota is ignored",
    );
  }
  if (quotaView.excludedFromDescriptorFingerprint !== true) {
    fail(
      "shareUserQuotaView must stay out of the descriptor fingerprint; consumption must not force a Router resync",
    );
  }
  if (quotaView.clientMayRederive !== false) {
    fail(
      "shareUserQuotaView must declare that the client reads the Server view instead of re-deriving it",
    );
  }

  const contractSource = fs.readFileSync(
    "src/domain/sharing/router_contract.rs",
    "utf8",
  );
  if (!contractSource.includes("pub struct ShareUserQuotaView")) {
    fail("src/domain/sharing/router_contract.rs is missing ShareUserQuotaView");
  }
  for (const field of quotaView.fields || []) {
    const snake = field.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
    if (!contractSource.includes(`pub ${snake}:`)) {
      fail(`ShareUserQuotaView is missing contracted field ${field}`);
    }
  }
  const fingerprintStart = contractSource.indexOf(
    "fn static_descriptor_projection(",
  );
  const fingerprintSource =
    fingerprintStart < 0 ? "" : contractSource.slice(fingerprintStart);
  if (!fingerprintSource.includes(`"${quotaView.field}"`)) {
    fail(
      `static_descriptor_projection must strip ${quotaView.field} from the fingerprint`,
    );
  }

  const shareSource = fs.readFileSync("src/domain/sharing/shares.rs", "utf8");
  if (!shareSource.includes("fn quota_view(")) {
    fail(
      "src/domain/sharing/shares.rs must derive the per-grant quota view in the domain layer",
    );
  }

  const normalizePath = "web-src/src/utils/shareRecordNormalize.ts";
  if (fs.existsSync(normalizePath)) {
    const normalizeSource = fs.readFileSync(normalizePath, "utf8");
    if (!normalizeSource.includes(`"${quotaView.field}"`) &&
        !normalizeSource.includes(`raw.${quotaView.field}`)) {
      fail(`${normalizePath} does not read the Server ${quotaView.field} view`);
    }
  }
}

const dispatchPath = "src/api/invoke/dispatch.rs";
if (fs.existsSync(dispatchPath)) {
  const httpSource = fs.readFileSync(dispatchPath, "utf8");
  const dispatchStart = httpSource.indexOf("async fn web_invoke_dispatch(");
  const dispatchEnd = httpSource.length;
  if (dispatchStart < 0 || dispatchEnd < 0 || dispatchEnd <= dispatchStart) {
    fail(`${dispatchPath} must expose a parseable web_invoke_dispatch block`);
  } else {
    const dispatchSource = httpSource.slice(dispatchStart, dispatchEnd);
    const dispatchCommands = new Set();
    const armPattern = /^\s*((?:"[^"]+"\s*(?:\|\s*)?)+)\s*=>/gm;
    for (const match of dispatchSource.matchAll(armPattern)) {
      for (const nameMatch of match[1].matchAll(/"([^"]+)"/g)) {
        dispatchCommands.add(nameMatch[1]);
      }
    }

    for (const command of commands) {
      const shouldDispatch = command.implemented && command.support !== "excluded";
      if (shouldDispatch && !dispatchCommands.has(command.name)) {
        fail(`implemented command ${command.name} is missing from web_invoke_dispatch`);
      }
      if (!shouldDispatch && dispatchCommands.has(command.name)) {
        fail(`command ${command.name} is dispatched but not marked implemented`);
      }
    }

    for (const commandName of dispatchCommands) {
      if (!commandByName.has(commandName)) {
        fail(`web_invoke_dispatch exposes unregistered command ${commandName}`);
      }
    }
  }
}

const forbiddenUiAutomationPackages = [
  "playwright",
  "@playwright/test",
  "cypress",
  "puppeteer",
  "selenium-webdriver",
];
for (const packageFile of ["package.json", "web-src/package.json"]) {
  if (!fs.existsSync(packageFile)) continue;
  const pkg = JSON.parse(fs.readFileSync(packageFile, "utf8"));
  const deps = {
    ...(pkg.dependencies || {}),
    ...(pkg.devDependencies || {}),
    ...(pkg.optionalDependencies || {}),
  };
  for (const name of forbiddenUiAutomationPackages) {
    if (Object.hasOwn(deps, name)) {
      fail(`${packageFile} must not depend on UI automation package ${name}`);
    }
  }
}

for (const lockFile of ["package-lock.json", "web-src/package-lock.json"]) {
  if (!fs.existsSync(lockFile)) continue;
  const lock = JSON.parse(fs.readFileSync(lockFile, "utf8"));
  for (const packagePath of Object.keys(lock.packages || {})) {
    const name = packagePath.split("node_modules/").pop();
    if (forbiddenUiAutomationPackages.includes(name)) {
      fail(`${lockFile} must not lock UI automation package ${name}`);
    }
  }
}

const webSrc = "web-src";
if (fs.existsSync(webSrc)) {
  const registered = new Set(commands.map((command) => command.name));
  const files = [];
  const stack = [webSrc];
  while (stack.length) {
    const current = stack.pop();
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const fullPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== "node_modules" && entry.name !== "dist") stack.push(fullPath);
        continue;
      }
      if (/\.(ts|tsx|js|jsx)$/.test(entry.name)) files.push(fullPath);
    }
  }
  const pattern = /invokeCommand(?:<[^>]+>)?\(\s*["']([^"']+)["']/g;
  for (const file of files) {
    const source = fs.readFileSync(file, "utf8");
    if (source.includes("new EventSource(")) {
      fail(`${file} must use authenticated fetch streaming instead of EventSource`);
    }
    if (/web-api\/(?:events|admin\/upgrade\/stream)[^"'`]*[?&](?:token|accessToken)=/.test(source)) {
      fail(`${file} leaks a client web token through an SSE URL`);
    }
    for (const match of source.matchAll(pattern)) {
      if (!registered.has(match[1])) {
        fail(`${file} invokes unregistered command ${match[1]}`);
      }
    }
  }
}

const routerDir = process.env.CC_SWITCH_ROUTER_DIR || "../cc-switch-router";
const routerProxy = path.join(routerDir, "src/proxy.rs");
if (fs.existsSync(routerProxy)) {
  const source = fs.readFileSync(routerProxy, "utf8");
  for (const marker of [
    'path.starts_with("/web-api/")',
    "!is_public_client_web_path(path)",
    '"/web-api/admin/upgrade/stream"',
    '"/web-api/admin/upgrade/status"',
    '"/web-api/admin/logs/tail"',
    "has_client_web_query_token",
    '"query-token-not-allowed"',
  ]) {
    if (!source.includes(marker)) {
      fail(`router client web policy is missing ${marker}`);
    }
  }
}

if (!process.exitCode) {
  console.log(
    `web-runtime-contract ok features=${featureIds.size} commands=${commands.length}`,
  );
}
