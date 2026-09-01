#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const allowedAttributionFiles = new Set([
  "SOURCE_PROVENANCE.json",
  "THIRD_PARTY_NOTICES.md",
]);
const ignoredDirectories = new Set([
  ".git",
  "node_modules",
  "target",
  "web-dist",
]);
const auditFiles = new Set([
  "scripts/audit/audit-source-provenance.mjs",
  "scripts/audit/source-provenance.test.mjs",
]);
const historicalOwner = ["farion", "1231"].join("");
const historicalRepository = [historicalOwner, "cc-switch"].join("/");
const technicalDependencyPatterns = Object.freeze([
  ["external Provider checkout root", /CC_SWITCH_PROVIDER_AUDIT_ROOT/],
  ["external Provider baseline", /upstream-provider-source-baseline[.]json/],
  ["external Provider audit", /audit-upstream-provider-baseline/],
  ["retired import ledger", /UPSTREAM_IMPORT[.]md/],
]);

function walkFiles(root) {
  const files = [];
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      if (entry.isDirectory() && ignoredDirectories.has(entry.name)) continue;
      const absolutePath = path.join(current, entry.name);
      if (entry.isDirectory()) stack.push(absolutePath);
      else if (entry.isFile()) files.push(absolutePath);
    }
  }
  return files.sort();
}

function relativePath(root, absolutePath) {
  return path.relative(root, absolutePath).replaceAll(path.sep, "/");
}

function readText(absolutePath) {
  const content = fs.readFileSync(absolutePath);
  return content.includes(0) ? null : content.toString("utf8");
}

export function technicalDependencyViolations(pathName, source) {
  if (pathName.startsWith("docs/history/") || auditFiles.has(pathName)) return [];
  const violations = [];
  for (const [label, pattern] of technicalDependencyPatterns) {
    if (pattern.test(source)) violations.push(`${pathName}: ${label}`);
  }
  return violations;
}

export function auditSourceProvenance(root = repoRoot) {
  const violations = [];
  const originPattern = new RegExp(
    historicalRepository.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
    "i",
  );

  for (const absolutePath of walkFiles(root)) {
    const pathName = relativePath(root, absolutePath);
    const source = readText(absolutePath);
    if (source === null) continue;
    if (originPattern.test(source) && !allowedAttributionFiles.has(pathName)) {
      violations.push(`${pathName}: historical repository attribution is not allowlisted`);
    }
    violations.push(...technicalDependencyViolations(pathName, source));
  }

  const provenancePath = path.join(root, "SOURCE_PROVENANCE.json");
  const noticePath = path.join(root, "THIRD_PARTY_NOTICES.md");
  const licensePath = path.join(root, "LICENSE");
  for (const requiredPath of [provenancePath, noticePath, licensePath]) {
    if (!fs.existsSync(requiredPath)) {
      violations.push(`${relativePath(root, requiredPath)}: required compliance file missing`);
    }
  }
  if (!fs.existsSync(provenancePath)) return violations;

  const provenance = JSON.parse(fs.readFileSync(provenancePath, "utf8"));
  if (provenance.projectLicense !== "MIT") {
    violations.push("SOURCE_PROVENANCE.json: projectLicense must be MIT");
  }
  if (
    provenance.policy?.runtimeAndBuildInputsMustBeRepositoryOwned !== true ||
    provenance.policy?.historicalAttributionIsNotATechnicalDependency !== true
  ) {
    violations.push("SOURCE_PROVENANCE.json: repository-owned input policy missing");
  }

  const historical = (provenance.adaptedSources ?? []).find(
    (entry) => entry.repository?.toLowerCase().endsWith(historicalRepository),
  );
  if (!historical || historical.license !== "MIT") {
    violations.push("SOURCE_PROVENANCE.json: MIT historical attribution missing");
  } else {
    if (historical.technicalInput !== false) {
      violations.push("SOURCE_PROVENANCE.json: historical source must not be a technical input");
    }
    if (historical.notice !== "THIRD_PARTY_NOTICES.md") {
      violations.push("SOURCE_PROVENANCE.json: historical source notice is not local");
    }
    for (const sourcePath of historical.reviewBoundary?.sources ?? []) {
      const absolutePath = path.join(root, sourcePath);
      if (!fs.existsSync(absolutePath)) {
        violations.push(`${sourcePath}: reviewed adaptation source missing`);
        continue;
      }
      const source = fs.readFileSync(absolutePath, "utf8");
      if (
        !source.includes("SOURCE_PROVENANCE.json") ||
        !source.includes("#[cfg(test)]")
      ) {
        violations.push(`${sourcePath}: provenance pointer or local tests missing`);
      }
    }
  }
  for (const entry of provenance.adaptedSources ?? []) {
    if (entry.technicalInput !== false) {
      violations.push(`SOURCE_PROVENANCE.json: ${entry.id} must declare technicalInput=false`);
    }
  }
  return violations;
}

function main() {
  const violations = auditSourceProvenance();
  if (violations.length > 0) {
    throw new Error(`Source provenance violations:\n${violations.join("\n")}`);
  }
  console.log("source provenance ok: attribution isolated from technical inputs");
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main();
}
