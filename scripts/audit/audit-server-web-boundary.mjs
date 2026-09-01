#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);

const entryPath = "web-src/src/main.tsx";
const sourceExtensions = Object.freeze([
  "",
  ".ts",
  ".tsx",
  ".js",
  ".jsx",
  ".json",
  ".css",
]);

const forbiddenExistingPaths = Object.freeze([
  "web-src/src/components/deeplink",
  "web-src/src/components/mcp",
  "web-src/src/components/prompts",
  "web-src/src/components/sessions",
  "web-src/src/components/skills",
  "web-src/src/shims/tauri-apps",
  "web-src/src/components/providers/AddProviderDialog.tsx",
  "web-src/src/components/providers/EditProviderDialog.tsx",
  "web-src/src/components/providers/ProviderList.tsx",
  "web-src/src/components/providers/forms/ProviderForm.tsx",
  "web-src/src/hooks/useMcp.ts",
  "web-src/src/hooks/useSkills.ts",
  "web-src/src/hooks/useTauriEvent.ts",
  "web-src/src/icons/extracted/hermes.png",
  "web-src/src/icons/extracted/openclaw.svg",
  "web-src/src/icons/extracted/opencode.svg",
  "web-src/src/icons/extracted/opencode-logo-light.svg",
]);

const requiredReachablePaths = Object.freeze([
  "web-src/src/ServerApp.tsx",
  "web-src/src/server/providerRegistry.ts",
  "web-src/src/server/directProviderPresets.ts",
  "web-src/src/server/providers/bundles/ProviderBundlesPage.tsx",
  "web-src/src/server/providers/bundles/ProviderBundleEditor.tsx",
]);

const forbiddenSourcePatterns = Object.freeze([
  ["Tauri runtime", /@tauri-apps\b|\b(?:isTauri|invokeTauri|TauriEvent)\b/i],
  ["desktop-only Provider surface", /\b(?:OpenCode|OpenClaw|Hermes|ClaudeDesktop)\b|claude-desktop/i],
  ["MCP surface", /\bMcp[A-Z][A-Za-z]*\b|\bmcp_servers\b/],
  ["cloud config transfer", /\bWebDav[A-Za-z]*\b|\bwebdav_sync\b|\bs3_sync\b/i],
  ["affiliate or campaign URL", /[?&](?:utm_[a-z_]+|aff(?:iliate)?(?:_?id)?|partner_?id)=/i],
  ["static promotion metadata", /\b(?:partnerPromotionKey|promotionCode|promoCode|affiliateCode)\b/],
]);

function toRelative(root, absolutePath) {
  return path.relative(root, absolutePath).replaceAll(path.sep, "/");
}

function materialPathExists(absolutePath) {
  if (!fs.existsSync(absolutePath)) return false;
  const stat = fs.statSync(absolutePath);
  if (!stat.isDirectory()) return true;
  return fs.readdirSync(absolutePath, { withFileTypes: true }).some((entry) =>
    entry.isDirectory()
      ? materialPathExists(path.join(absolutePath, entry.name))
      : true,
  );
}

export function extractModuleSpecifiers(source) {
  const specifiers = new Set();
  const patterns = [
    /^\s*(?:import|export)\s+(?:type\s+)?[\w*{},\s]+?\sfrom\s*["']([^"']+)["']/gm,
    /^\s*import\s*["']([^"']+)["']/gm,
    /\bimport\s*\(\s*["']([^"']+)["']\s*\)/g,
  ];
  for (const pattern of patterns) {
    for (const match of source.matchAll(pattern)) specifiers.add(match[1]);
  }
  return [...specifiers].sort();
}

function resolveLocalModule(root, importer, rawSpecifier) {
  const specifier = rawSpecifier.replace(/[?#].*$/, "");
  if (!specifier.startsWith(".") && !specifier.startsWith("@/")) return null;
  const unresolved = specifier.startsWith("@/")
    ? path.join(root, "web-src/src", specifier.slice(2))
    : path.resolve(path.dirname(importer), specifier);
  if (!unresolved.startsWith(`${root}${path.sep}`)) return undefined;

  for (const extension of sourceExtensions) {
    const candidate = `${unresolved}${extension}`;
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
      return candidate;
    }
  }
  for (const extension of sourceExtensions.slice(1)) {
    const candidate = path.join(unresolved, `index${extension}`);
    if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
      return candidate;
    }
  }
  return undefined;
}

export function buildProductionGraph(root = repoRoot) {
  const entry = path.join(root, entryPath);
  const reachable = new Set();
  const unresolved = [];
  const externalSpecifiers = new Map();
  const queue = [entry];

  while (queue.length > 0) {
    const current = queue.shift();
    if (reachable.has(current)) continue;
    if (!fs.existsSync(current)) {
      unresolved.push(`${toRelative(root, current)}: entry does not exist`);
      continue;
    }
    reachable.add(current);
    if (!/[.](?:[cm]?[jt]sx?|css)$/.test(current)) continue;
    const source = fs.readFileSync(current, "utf8");
    for (const specifier of extractModuleSpecifiers(source)) {
      const resolved = resolveLocalModule(root, current, specifier);
      if (resolved === null) {
        const importers = externalSpecifiers.get(specifier) ?? [];
        importers.push(toRelative(root, current));
        externalSpecifiers.set(specifier, importers);
      } else if (resolved === undefined) {
        unresolved.push(
          `${toRelative(root, current)}: unresolved local import ${specifier}`,
        );
      } else if (!reachable.has(resolved)) {
        queue.push(resolved);
      }
    }
  }

  return { reachable, unresolved, externalSpecifiers };
}

export function sourceContentViolations(relativePath, source) {
  const violations = [];
  for (const [label, pattern] of forbiddenSourcePatterns) {
    if (pattern.test(source)) violations.push(`${relativePath}: ${label}`);
  }
  return violations;
}

export function auditServerWebBoundary(root = repoRoot) {
  const violations = [];
  for (const relativePath of forbiddenExistingPaths) {
    if (materialPathExists(path.join(root, relativePath))) {
      violations.push(`${relativePath}: non-Server Web surface exists`);
    }
  }

  const graph = buildProductionGraph(root);
  violations.push(...graph.unresolved);
  for (const [specifier, importers] of graph.externalSpecifiers) {
    if (specifier.startsWith("@tauri-apps/")) {
      violations.push(
        `${importers.join(", ")}: forbidden external import ${specifier}`,
      );
    }
  }

  const reachableRelative = new Set(
    [...graph.reachable].map((absolutePath) => toRelative(root, absolutePath)),
  );
  for (const requiredPath of requiredReachablePaths) {
    if (!reachableRelative.has(requiredPath)) {
      violations.push(`${requiredPath}: required Server Web boundary is unreachable`);
    }
  }
  for (const absolutePath of graph.reachable) {
    if (!/[.](?:[cm]?[jt]sx?|json|css)$/.test(absolutePath)) continue;
    const relativePath = toRelative(root, absolutePath);
    violations.push(
      ...sourceContentViolations(
        relativePath,
        fs.readFileSync(absolutePath, "utf8"),
      ),
    );
  }
  return violations;
}

function main() {
  const violations = auditServerWebBoundary();
  if (violations.length > 0) {
    throw new Error(`Server Web boundary violations:\n${violations.join("\n")}`);
  }
  const graph = buildProductionGraph();
  console.log(
    `server web boundary ok: reachable_modules=${graph.reachable.size}, unresolved=0`,
  );
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main();
}
