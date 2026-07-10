#!/usr/bin/env node
/**
 * Sync selected desktop UI files from /data/projects/cc-switch into web-src.
 *
 * Usage:
 *   node scripts/sync/sync-desktop-ui.mjs [--check] [path...]
 *
 * Default paths mirror the Phase U12 direct-port scope.
 */
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const desktopRoot = process.env.CC_SWITCH_DESKTOP_ROOT || "/data/projects/cc-switch";
const serverWebSrc = path.resolve("web-src/src");

// server does not run vitest; desktop test files are intentionally not synced
const skipSuffixes = [".test.ts", ".test.tsx", ".spec.ts", ".spec.tsx"];

function shouldSkipSync(file) {
  return skipSuffixes.some((suffix) => file.endsWith(suffix));
}

export const defaultPaths = [
  "index.css",
  "i18n/index.ts",
  "i18n/locales/en.json",
  "i18n/locales/zh.json",
  "i18n/locales/zh-TW.json",
  "i18n/locales/ja.json",
  "components/ui",
  "components/providers/forms",
  "components/providers/AddProviderDialog.tsx",
  "components/providers/ProviderEmptyState.tsx",
  "components/providers/ProviderList.tsx",
  "components/providers/ProviderCard.tsx",
  "components/providers/ProviderActions.tsx",
  "components/providers/ProviderEmptyState.tsx",
  "components/providers/ProviderHealthBadge.tsx",
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
  "lib/utils.ts",
  "lib/query/queryClient.ts",
];

export const serverLocalOverrides = [
  "i18n/locales/en.json",
  "i18n/locales/zh.json",
  "i18n/locales/zh-TW.json",
  "i18n/locales/ja.json",
  "components/providers/forms/AntigravityOAuthSection.tsx",
  "components/providers/forms/CodexOAuthSection.tsx",
  "components/providers/forms/CopilotAuthSection.tsx",
  "components/providers/forms/CursorOAuthSection.tsx",
  "components/providers/forms/GeminiOAuthSection.tsx",
  "components/providers/forms/KiroOAuthSection.tsx",
  "components/providers/forms/hooks/useManagedAuth.ts",
  "components/providers/forms/ProviderForm.tsx",
  "components/providers/forms/ProviderPresetSelector.tsx",
  "components/providers/ProviderShareSection.tsx",
  "components/ConfirmDialog.tsx",
  "components/common/FullScreenPanel.tsx",
  "components/settings/SettingsPage.tsx",
  "components/settings/ShareSettingsTab.tsx",
  "components/settings/ServerVersionSettings.tsx",
  "components/settings/ClientTunnelSettingsPanel.tsx",
  "components/share/EditShareDialog.tsx",
  "components/share/ImportSharesModal.tsx",
  "components/share/OwnerChangeModal.tsx",
  "components/share/ShareEmptyState.tsx",
  "components/share/ShareExportModal.tsx",
  "components/share/SharePage.tsx",
  "components/usage/UsageMiniMetric.tsx",
  "components/usage/UsageTabs.tsx",
  "components/usage/index.ts",
];

export const serverExcludedFromSync = [
  "components/universal",
  "config/universalProviderPresets.ts",
  "components/share/ShareOwnerChangeEmailDialog.tsx",
  "components/share/ShareOwnerLoginDialog.tsx",
  "components/share/ShareRouterBar.tsx",
  "components/settings/ShareEmailLoginCard.tsx",
  "components/settings/ImportExportPanel.tsx",
];

const serverLocalOverrideSet = new Set(serverLocalOverrides);
const serverExcludedFromSyncSet = new Set(serverExcludedFromSync);

function copyFile(src, dest, checkOnly) {
  if (!fs.existsSync(src)) {
    throw new Error(`missing desktop source: ${src}`);
  }
  if (shouldSkipSync(src)) {
    return;
  }
  const relativeDest = toWebSrcRelative(dest);
  if (serverExcludedFromSyncSet.has(relativeDest)) {
    return;
  }
  if (!checkOnly && serverLocalOverrideSet.has(relativeDest)) {
    return;
  }
  if (checkOnly) {
    if (!fs.existsSync(dest)) {
      throw new Error(`missing server target: ${dest}`);
    }
    if (
      !serverLocalOverrideSet.has(relativeDest) &&
      !fs.readFileSync(src).equals(fs.readFileSync(dest))
    ) {
      throw new Error(`server copy drifted from desktop: ${dest}`);
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
    if (entry.isFile() && shouldSkipSync(entry.name)) {
      continue;
    }
    copyTree(path.join(src, entry.name), path.join(dest, entry.name), checkOnly);
  }
}

function toWebSrcRelative(file) {
  return path.relative(serverWebSrc, file).split(path.sep).join("/");
}

function walkFiles(root) {
  if (!fs.existsSync(root)) {
    return [];
  }
  const stat = fs.statSync(root);
  if (stat.isFile()) {
    return [root];
  }
  return fs
    .readdirSync(root, { withFileTypes: true })
    .flatMap((entry) => walkFiles(path.join(root, entry.name)));
}

function checkUnexpectedServerFiles(relativePath) {
  const src = path.join(desktopRoot, "src", relativePath);
  const dest = path.join(serverWebSrc, relativePath);
  if (!fs.existsSync(src) || !fs.existsSync(dest) || fs.statSync(dest).isFile()) {
    return;
  }
  for (const file of walkFiles(dest)) {
    if (shouldSkipSync(file)) {
      continue;
    }
    const source = path.join(src, path.relative(dest, file));
    const relativeDest = toWebSrcRelative(file);
    if (!fs.existsSync(source) && !serverLocalOverrideSet.has(relativeDest)) {
      throw new Error(`unexpected server-local file in synced tree: ${file}`);
    }
  }
}

function main() {
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
      if (checkOnly) {
        checkUnexpectedServerFiles(relativePath);
      }
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
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
