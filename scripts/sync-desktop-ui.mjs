#!/usr/bin/env node
/**
 * Sync selected desktop UI files from /data/projects/cc-switch into web-src.
 *
 * Usage:
 *   node scripts/sync-desktop-ui.mjs [--check] [path...]
 *
 * Default paths mirror the Phase U12 direct-port scope.
 */
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const desktopRoot = process.env.CC_SWITCH_DESKTOP_ROOT || "/data/projects/cc-switch";
const serverWebSrc = path.resolve("web-src/src");

const defaultPaths = [
  "index.css",
  "i18n/index.ts",
  "i18n/locales/en.json",
  "i18n/locales/zh.json",
  "i18n/locales/zh-TW.json",
  "i18n/locales/ja.json",
  "components/ui",
  "components/providers/forms",
  "components/providers/AddProviderDialog.tsx",
  "components/providers/EditProviderDialog.tsx",
  "components/providers/ProviderList.tsx",
  "components/providers/ProviderCard.tsx",
  "components/providers/ProviderActions.tsx",
  "components/providers/ProviderEmptyState.tsx",
  "components/providers/ProviderPresetSelector.tsx",
  "components/providers/ProviderHealthIndicator.tsx",
  "components/ConfirmDialog.tsx",
  "components/JsonEditor.tsx",
  "components/ProviderIcon.tsx",
  "components/BrandIcons.tsx",
  "components/ClaudeOauthQuotaFooter.tsx",
  "components/CodexOauthQuotaFooter.tsx",
  "components/GeminiOauthQuotaFooter.tsx",
  "components/CopilotQuotaFooter.tsx",
  "components/CursorOauthQuotaFooter.tsx",
  "components/KiroOauthQuotaFooter.tsx",
  "components/AntigravityOauthQuotaFooter.tsx",
  "components/OllamaQuotaFooter.tsx",
  "components/SubscriptionQuotaFooter.tsx",
  "components/settings/SettingsPage.tsx",
  "components/settings/LanguageSettings.tsx",
  "components/settings/ThemeSettings.tsx",
  "components/share",
  "components/usage",
  "components/universal",
  "lib/utils.ts",
  "lib/query/queryClient.ts",
];

function copyFile(src, dest, checkOnly) {
  if (!fs.existsSync(src)) {
    throw new Error(`missing desktop source: ${src}`);
  }
  if (checkOnly) {
    if (!fs.existsSync(dest)) {
      throw new Error(`missing server target: ${dest}`);
    }
    const srcStat = fs.statSync(src);
    const destStat = fs.statSync(dest);
    if (srcStat.mtimeMs > destStat.mtimeMs + 1000) {
      throw new Error(`server copy is older than desktop: ${dest}`);
    }
    return;
  }
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
  console.log(`synced ${path.relative(desktopRoot, src)} -> ${path.relative(process.cwd(), dest)}`);
}

function copyTree(src, dest, checkOnly) {
  if (!fs.existsSync(src)) {
    throw new Error(`missing desktop source: ${src}`);
  }
  const stat = fs.statSync(src);
  if (stat.isFile()) {
    copyFile(src, dest, checkOnly);
    return;
  }
  for (const entry of fs.readdirSync(src, { withFileTypes: true })) {
    copyTree(path.join(src, entry.name), path.join(dest, entry.name), checkOnly);
  }
}

const args = process.argv.slice(2);
const checkOnly = args.includes("--check");
const paths = args.filter((arg) => !arg.startsWith("--"));
const selected = paths.length ? paths : defaultPaths;

let failures = 0;
for (const relativePath of selected) {
  const src = path.join(desktopRoot, "src", relativePath);
  const dest = path.join(serverWebSrc, relativePath);
  try {
    copyTree(src, dest, checkOnly);
  } catch (error) {
    failures += 1;
    console.error(error instanceof Error ? error.message : String(error));
  }
}

if (failures) {
  console.error(`sync-desktop-ui failed: ${failures} path(s)`);
  process.exit(1);
}

console.log(checkOnly ? "sync-desktop-ui check ok" : "sync-desktop-ui complete");
