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
