#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const apps = ["claude", "codex", "gemini"];

function usage() {
  console.error("Usage: node scripts/sync/import-desktop-export.mjs <desktop-export.json> [--out-dir DIR]");
  process.exit(2);
}

const args = process.argv.slice(2);
const inputPath = args.find((arg) => !arg.startsWith("--"));
if (!inputPath) usage();
const outDirIndex = args.indexOf("--out-dir");
const outDir = outDirIndex >= 0 ? args[outDirIndex + 1] : null;
if (outDirIndex >= 0 && !outDir) usage();

const payload = JSON.parse(fs.readFileSync(inputPath, "utf8"));
const bundle = {
  providers: normalizeProviders(payload),
  universalProviders: normalizeArray(firstDefined(payload.universalProviders, payload.universal_providers, payload.universal)),
  accounts: normalizeArray(firstDefined(payload.accounts, payload.authAccounts, payload.oauthAccounts)),
  shares: normalizeArray(firstDefined(payload.shares, payload.shareConfigs)),
  warnings: [],
};

if (!bundle.providers.length) bundle.warnings.push("no providers found in desktop export");
if (!bundle.accounts.length) bundle.warnings.push("no accounts found; server accounts may need manual import");
if (!bundle.shares.length) bundle.warnings.push("no shares found in desktop export");

const summary = {
  providers: bundle.providers.length,
  universalProviders: bundle.universalProviders.length,
  accounts: bundle.accounts.length,
  shares: bundle.shares.length,
  warnings: bundle.warnings,
};

if (outDir) {
  fs.mkdirSync(outDir, {recursive: true});
  writeJson(path.join(outDir, "providers-import.json"), {providers: bundle.providers});
  writeJson(path.join(outDir, "universal-providers-import.json"), {providers: bundle.universalProviders});
  writeJson(path.join(outDir, "accounts-import.json"), {accounts: bundle.accounts});
  writeJson(path.join(outDir, "shares-import.json"), {shares: bundle.shares});
  writeJson(path.join(outDir, "migration-summary.json"), summary);
} else {
  console.log(JSON.stringify({summary, bundle}, null, 2));
}

function normalizeProviders(value) {
  const providers = [];
  for (const item of normalizeArray(value.providers)) {
    const normalized = normalizeStoredProvider(item, item.app);
    if (normalized) providers.push(normalized);
  }
  for (const app of apps) {
    for (const key of [`${app}Providers`, `${app}_providers`, app]) {
      for (const item of normalizeArray(value[key])) {
        const normalized = normalizeStoredProvider(item, app);
        if (normalized) providers.push(normalized);
      }
    }
  }
  return dedupeProviders(providers);
}

function normalizeStoredProvider(item, appHint) {
  if (!item || typeof item !== "object") return null;
  const app = normalizeApp(item.app || appHint);
  const provider = item.provider && typeof item.provider === "object" ? item.provider : item;
  if (!app || !provider.id) return null;
  return {app, provider};
}

function normalizeApp(value) {
  const app = String(value || "").toLowerCase();
  return apps.includes(app) ? app : null;
}

function normalizeArray(value) {
  if (Array.isArray(value)) return value;
  if (value && typeof value === "object") return Object.values(value);
  return [];
}

function firstDefined(...values) {
  return values.find((value) => value !== undefined && value !== null);
}

function dedupeProviders(providers) {
  const seen = new Set();
  return providers.filter((item) => {
    const key = `${item.app}:${item.provider.id}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}
