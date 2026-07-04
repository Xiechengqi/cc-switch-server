#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const ROOT = process.cwd();
const WEB_SRC = path.join(ROOT, "web-src", "src");
const COMPONENT_DIR = path.join(WEB_SRC, "components");
const MAX_TOTAL = Number(process.env.WEB_I18N_MAX_ENGLISH_LITERALS || 80);

const ignoredText = new Set([
  "API",
  "ID",
  "JSON",
  "OpenAI",
  "Promise",
  "URL",
]);

const ignoredPropNames = new Set(["label", "title", "subtitle", "placeholder", "aria-label"]);
const ignoredUppercasePropComponents = new Set([
  "ActionButton",
  "AppBadge",
  "DataSourceChip",
  "EmptyRow",
  "FormFooter",
  "IconAction",
  "KeyValue",
  "LoadingBlock",
  "ModalFooter",
  "ProviderJsonField",
  "ReadinessFlag",
  "SectionHeader",
  "SimpleModal",
  "SummaryTile",
  "TextField",
]);

const files = [
  ...fs.readdirSync(COMPONENT_DIR)
    .filter((file) => file.endsWith(".tsx"))
    .map((file) => path.join(COMPONENT_DIR, file)),
  path.join(WEB_SRC, "App.tsx"),
];

const findings = [];
for (const file of files) {
  const source = fs.readFileSync(file, "utf8");
  const lineStarts = [0];
  for (let index = 0; index < source.length; index += 1) {
    if (source[index] === "\n") lineStarts.push(index + 1);
  }

  for (const match of source.matchAll(/>\s*([A-Z][A-Za-z][A-Za-z0-9 ,:;()_./%+&'’?-]{2,})\s*</g)) {
    const value = match[1].trim();
    if (shouldIgnoreText(value)) continue;
    findings.push({ file, line: lineOf(lineStarts, match.index), value, kind: "jsx-text" });
  }

  for (const match of source.matchAll(/\b(label|title|subtitle|placeholder|aria-label)=["']([A-Z][^"'{}]{2,})["']/g)) {
    const [, prop, value] = match;
    if (shouldIgnoreText(value)) continue;
    const component = componentNameBefore(source, match.index);
    if (component && ignoredPropNames.has(prop) && ignoredUppercasePropComponents.has(component)) continue;
    findings.push({ file, line: lineOf(lineStarts, match.index), value, kind: `${prop}-prop` });
  }
}

const byFile = new Map();
for (const finding of findings) {
  const relative = path.relative(ROOT, finding.file);
  byFile.set(relative, (byFile.get(relative) || 0) + 1);
}

console.log(
  `web-i18n-literals total=${findings.length} max=${MAX_TOTAL} files=${[...byFile.entries()]
    .map(([file, count]) => `${file}:${count}`)
    .join(", ")}`,
);

if (findings.length > MAX_TOTAL) {
  for (const finding of findings.slice(0, 80)) {
    console.error(
      `${path.relative(ROOT, finding.file)}:${finding.line}: ${finding.kind}: ${JSON.stringify(finding.value)}`,
    );
  }
  throw new Error(`web i18n literal count ${findings.length} exceeds max ${MAX_TOTAL}`);
}

function shouldIgnoreText(value) {
  if (ignoredText.has(value)) return true;
  if (/^[A-Z0-9_./ -]+$/.test(value) && value.length <= 12) return true;
  if (/^(Claude|Codex|Gemini|Kiro|Copilot|GitHub|AWS|OAuth|OpenRouter|Ollama)(?:\b|$)/.test(value)) {
    return true;
  }
  return false;
}

function componentNameBefore(source, index) {
  const before = source.slice(0, index);
  const match = before.match(/<([A-Za-z][A-Za-z0-9.]*)[^<>]*$/);
  return match?.[1]?.split(".").pop() || null;
}

function lineOf(lineStarts, index = 0) {
  let low = 0;
  let high = lineStarts.length - 1;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    if (lineStarts[middle] <= index) low = middle + 1;
    else high = middle - 1;
  }
  return high + 1;
}
