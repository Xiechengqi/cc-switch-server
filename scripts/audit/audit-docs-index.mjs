#!/usr/bin/env node
// Docs hygiene audit:
//   1. every markdown link inside docs/, README.md, AGENTS.md and PROTOCOL_EVIDENCE.md
//      resolves to a file that exists;
//   2. every content doc under docs/ is registered in the docs/README.md index;
//   3. every docs/history/*.md carries the archived banner.
// Run: node scripts/audit/audit-docs-index.mjs

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const docsDir = path.join(repoRoot, "docs");
const indexPath = path.join(docsDir, "README.md");

// Local-only (gitignored) or data directories that are intentionally not indexed as docs.
const UNINDEXED = new Set(["docs/README.md", "docs/remaining-work-index.md"]);
const UNSCANNED_DIRS = new Set(["docs/provider-fixtures"]);

const failures = [];

function walk(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    const rel = path.relative(repoRoot, full).split(path.sep).join("/");
    if (entry.isDirectory()) {
      if (UNSCANNED_DIRS.has(rel)) continue;
      walk(full, out);
    } else if (entry.name.endsWith(".md")) {
      out.push(rel);
    }
  }
  return out;
}

const docFiles = fs.existsSync(docsDir) ? walk(docsDir).sort() : [];
const rootDocs = ["README.md", "AGENTS.md", "PROTOCOL_EVIDENCE.md", "THIRD_PARTY_NOTICES.md"].filter((rel) =>
  fs.existsSync(path.join(repoRoot, rel)),
);

// --- 1. link resolution -------------------------------------------------------
const LINK_RE = /\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g;
let linkCount = 0;

for (const rel of [...docFiles, ...rootDocs]) {
  const text = fs.readFileSync(path.join(repoRoot, rel), "utf8");
  for (const match of text.matchAll(LINK_RE)) {
    const raw = match[1];
    if (/^(https?:|mailto:|#)/.test(raw)) continue;
    const target = raw.split("#")[0];
    if (!target) continue;
    if (!/\.(md|json|jsonl|sh|mjs|rs|toml|service|ts|tsx)$/.test(target) && !target.endsWith("/")) {
      continue;
    }
    linkCount += 1;
    const resolved = path.resolve(path.dirname(path.join(repoRoot, rel)), target);
    if (!fs.existsSync(resolved)) {
      failures.push(`${rel}: broken link -> ${raw}`);
    }
  }
}

// --- 2. index completeness ----------------------------------------------------
if (!fs.existsSync(indexPath)) {
  failures.push("docs/README.md is missing; it is the required docs index");
} else {
  const index = fs.readFileSync(indexPath, "utf8");
  for (const rel of docFiles) {
    if (UNINDEXED.has(rel)) continue;
    const fromIndex = rel.replace(/^docs\//, "");
    if (!index.includes(`(${fromIndex})`)) {
      failures.push(`${rel}: not registered in docs/README.md (expected a link to \`${fromIndex}\`)`);
    }
  }
}

// --- 3. archive banners -------------------------------------------------------
for (const rel of docFiles.filter((f) => f.startsWith("docs/history/"))) {
  const text = fs.readFileSync(path.join(repoRoot, rel), "utf8");
  if (!text.includes("归档文档 · 只读 · 不代表当前实现")) {
    failures.push(`${rel}: missing the archived banner required for docs/history/`);
  }
}

if (failures.length > 0) {
  for (const line of failures) console.error(`[docs] ${line}`);
  console.error(`docs index audit failed with ${failures.length} problem(s)`);
  process.exit(1);
}

console.log(`docs index audit ok (files=${docFiles.length + rootDocs.length} links=${linkCount})`);
