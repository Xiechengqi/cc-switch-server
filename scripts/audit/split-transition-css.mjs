#!/usr/bin/env node
/**
 * Split web-src/src/styles.css transition layer into scoped modules and prune
 * zero-reference class rules. Run from repo root:
 *   node scripts/audit/split-transition-css.mjs
 */
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const stylesDir = path.join(root, "web-src/src/styles");
const stylesPath = path.join(root, "web-src/src/styles.css");
const srcRoot = path.join(root, "web-src/src");

const MODULES = {
  "providers.css": (selectors) =>
    selectors.some(
      (s) =>
        /\.provider-/.test(s) ||
        /\.failover-priority-badge/.test(s) ||
        /\.section-heading/.test(s) ||
        /\.section-title-row/.test(s) ||
        /\.compact-kv/.test(s) ||
        /\.modal-backdrop/.test(s) ||
        /\.provider-form-modal/.test(s) ||
        /\.segmented/.test(s) ||
        /\.wide-field/.test(s) ||
        (/@media/.test(s) &&
          /provider-|share-card > \.provider-actions/.test(s)),
    ),
  "auth-accounts.css": (selectors) =>
    selectors.some(
      (s) =>
        /\.account-/.test(s) ||
        /\.auth-center-/.test(s) ||
        /\.banked-/.test(s) ||
        /\.device-flow-/.test(s) ||
        /\.credential-badges/.test(s) ||
        /\.capability-flags/.test(s) ||
        /\.json-preview/.test(s) ||
        /\.json-editor-field/.test(s) ||
        /\.json-details/.test(s) ||
        /\.template-details/.test(s) ||
        /\.template-note/.test(s) ||
        /\.color-picker-/.test(s) ||
        /\.account-tool-/.test(s) ||
        /\.toggle-row/.test(s) ||
        /\.oauth-result/.test(s) ||
        /\.device-code-block/.test(s) ||
        /\.inline-link/.test(s) ||
        /\.owner-change-/.test(s) ||
        /\.owner-node/.test(s) ||
        /\.owner-step/.test(s) ||
        /\.owner-request-button/.test(s) ||
        /\.connect-copy-status/.test(s) ||
        /\.compact-empty/.test(s) ||
        /\.inline-empty/.test(s),
    ),
  "usage.css": (selectors) =>
    selectors.some((s) => /\.usage-/.test(s)),
  "modals.css": (selectors) =>
    selectors.some(
      (s) =>
        /\.simple-modal/.test(s) ||
        /\.modal-form-stack/.test(s) ||
        /\.modal-inline-footer/.test(s) ||
        /\.owner-change-form/.test(s),
    ),
  "universal.css": (selectors) =>
    selectors.some((s) => /\.universal-/.test(s)),
};

function collectSourceText(dir, excludedPaths) {
  let output = "";
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "i18n" || entry.name === "styles") {
        continue;
      }
      output += collectSourceText(fullPath, excludedPaths);
      continue;
    }
    if (/\.(tsx|ts)$/.test(entry.name) && !excludedPaths.has(fullPath)) {
      output += fs.readFileSync(fullPath, "utf8");
      output += "\n";
    }
  }
  return output;
}

function classIsReferenced(className, sourceText) {
  const patterns = [
    new RegExp(`className=\\{[^}]*["'\`]${className}(?:\\s|["'\`])`),
    new RegExp(`className=["'\`][^"'\`]*\\b${className}\\b`),
    new RegExp(`class:\\s*["'\`][^"'\`]*\\b${className}\\b`),
  ];
  return patterns.some((re) => re.test(sourceText));
}

function extractSelectors(ruleHead) {
  return ruleHead
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean);
}

function ruleReferencesOnlyDeadSelectors(selectors, sourceText) {
  const classNames = [];
  for (const selector of selectors) {
    for (const match of selector.matchAll(/\.(-?[_a-zA-Z]+[_a-zA-Z0-9-]*)/g)) {
      classNames.push(match[1]);
    }
  }
  if (classNames.length === 0) {
    return false;
  }
  return classNames.every((name) => !classIsReferenced(name, sourceText));
}

function tokenizeCss(input) {
  const tokens = [];
  let i = 0;
  while (i < input.length) {
    if (input.startsWith("@media", i)) {
      const brace = input.indexOf("{", i);
      const prelude = input.slice(i, brace).trim();
      let depth = 1;
      let j = brace + 1;
      while (j < input.length && depth > 0) {
        if (input[j] === "{") depth += 1;
        if (input[j] === "}") depth -= 1;
        j += 1;
      }
      tokens.push({
        kind: "at",
        prelude,
        body: input.slice(brace + 1, j - 1),
      });
      i = j;
      continue;
    }

    const nextBrace = input.indexOf("{", i);
    if (nextBrace === -1) {
      const tail = input.slice(i).trim();
      if (tail) {
        tokens.push({ kind: "raw", text: tail });
      }
      break;
    }
    const head = input.slice(i, nextBrace).trim();
    let depth = 1;
    let j = nextBrace + 1;
    while (j < input.length && depth > 0) {
      if (input[j] === "{") depth += 1;
      if (input[j] === "}") depth -= 1;
      j += 1;
    }
    tokens.push({
      kind: "rule",
      selectors: extractSelectors(head),
      body: input.slice(nextBrace, j),
    });
    i = j;
  }
  return tokens;
}

function serializeTokens(tokens) {
  return tokens
    .map((token) => {
      if (token.kind === "raw") {
        return token.text;
      }
      if (token.kind === "at") {
        const inner = serializeTokens(tokenizeCss(token.body));
        return `${token.prelude}{${inner}}`;
      }
      const head = token.selectors.join(",");
      return `${head}${token.body}`;
    })
    .join("");
}

function assignModule(token) {
  if (token.kind !== "rule" && token.kind !== "at") {
    return "core";
  }
  if (token.kind === "at") {
    const inner = tokenizeCss(token.body);
    const modules = new Set(inner.map((child) => assignModule(child)));
    modules.delete("core");
    if (modules.size === 1) {
      return [...modules][0];
    }
    return "core";
  }
  for (const [moduleName, matcher] of Object.entries(MODULES)) {
    if (matcher(token.selectors)) {
      return moduleName;
    }
  }
  return "core";
}

function splitTokens(tokens) {
  const buckets = { core: [] };
  for (const token of tokens) {
    const moduleName = assignModule(token);
    buckets[moduleName] ??= [];
    buckets[moduleName].push(token);
  }
  return buckets;
}

function pruneTokens(tokens, sourceText) {
  const output = [];
  for (const token of tokens) {
    if (token.kind === "raw") {
      output.push(token);
      continue;
    }
    if (token.kind === "at") {
      const pruned = pruneTokens(tokenizeCss(token.body), sourceText);
      if (pruned.length === 0) {
        continue;
      }
      output.push({ ...token, body: serializeTokens(pruned) });
      continue;
    }
    if (ruleReferencesOnlyDeadSelectors(token.selectors, sourceText)) {
      continue;
    }
    output.push(token);
  }
  return output;
}

function countLines(text) {
  return text.split("\n").length - (text.endsWith("\n") ? 1 : 0);
}

const css = fs.readFileSync(stylesPath, "utf8");
const sourceText = collectSourceText(srcRoot, new Set([stylesPath]));
let tokens = tokenizeCss(css);
tokens = pruneTokens(tokens, sourceText);
const buckets = splitTokens(tokens);

fs.mkdirSync(stylesDir, { recursive: true });
const coreCss = serializeTokens(buckets.core ?? []);
fs.writeFileSync(stylesPath, `${coreCss.trim()}\n`);

for (const [fileName, moduleTokens] of Object.entries(buckets)) {
  if (fileName === "core" || moduleTokens.length === 0) {
    continue;
  }
  const outPath = path.join(stylesDir, fileName);
  const body = serializeTokens(moduleTokens).trim();
  fs.writeFileSync(outPath, `${body}\n`);
}

const mainPath = path.join(srcRoot, "main.tsx");
let main = fs.readFileSync(mainPath, "utf8");
for (const fileName of Object.keys(buckets)) {
  if (fileName === "core") {
    continue;
  }
  const importLine = `import "./styles/${fileName}";`;
  if (!main.includes(importLine)) {
    main = main.replace(
      'import "./styles.css";',
      `import "./styles.css";\n${importLine}`,
    );
  }
}
fs.writeFileSync(mainPath, main);

const report = {
  coreLines: countLines(coreCss),
  modules: Object.fromEntries(
    Object.entries(buckets)
      .filter(([name]) => name !== "core")
      .map(([name, toks]) => [name, countLines(serializeTokens(toks))]),
  ),
};
console.log(JSON.stringify(report, null, 2));
