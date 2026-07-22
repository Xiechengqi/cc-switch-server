#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const WEB_SRC = path.join(ROOT, "web-src", "src");
const requireFromWeb = createRequire(
  path.join(ROOT, "web-src", "package.json"),
);
const ts = requireFromWeb("typescript");

const MAX_TOTAL = Number(process.env.WEB_I18N_MAX_VISIBLE_LITERALS || 49);
const MAX_PARTIAL_KEYS = Number(process.env.WEB_I18N_MAX_PARTIAL_KEYS || 207);
const LANGUAGES = ["en", "zh", "zh-TW", "ja"];
const ACCOUNT_TRANSLATION_NAMESPACES = [
  "antigravityOauth",
  "claudeOauth",
  "codexOauth",
  "copilot",
  "cursorOauth",
  "deepseekAccount",
  "geminiOauth",
  "grokOauth",
  "kiroOauth",
];
const ACCOUNT_TRANSLATION_KEYS = [
  "accountCount",
  "notAuthenticated",
  "loggedInAccounts",
  "defaultAccount",
  "selected",
  "setAsDefault",
  "removeAccount",
  "selectAccount",
  "selectAccountPlaceholder",
  "useDefaultAccount",
  "addAnotherAccount",
  "waitingForBrowser",
  "openLinkHint",
  "copyLink",
  "openManually",
  "logoutAll",
  "retry",
];
const REQUIRED_COMPLETE_PREFIXES = [
  "accountAuth.",
  "serverProviderForm.",
  "provider.unsavedChanges.",
  "confirm.",
];
const REQUIRED_COMPLETE_KEYS = new Set([
  "provider.share.deleteConfirmTitle",
  "provider.share.deleteConfirmMessage",
  "provider.share.deleteRemember",
  "settings.serverVersion.rollback",
  "settings.serverVersion.rollbackConfirmTitle",
  "settings.serverVersion.rollbackConfirmMessage",
  "share.confirmDeleteTitle",
  "share.confirmDeleteMessage",
  "share.freeConfirm",
]);

const ignoredText = new Set(["API", "ID", "JSON", "OpenAI", "Promise", "URL"]);
const ignoredPropNames = new Set([
  "label",
  "title",
  "subtitle",
  "placeholder",
  "aria-label",
]);
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

const files = walk(WEB_SRC).filter(
  (file) =>
    /\.(ts|tsx)$/.test(file) &&
    !/\.(test|spec)\.(ts|tsx)$/.test(file) &&
    !file.includes(`${path.sep}i18n${path.sep}locales${path.sep}`) &&
    !file.includes(`${path.sep}i18n${path.sep}server-locales${path.sep}`),
);
const localeKeys = loadLocaleKeys();
const literalFindings = [];
const translationCalls = new Map();

for (const file of files) {
  const source = fs.readFileSync(file, "utf8");
  const lineStarts = lineStartOffsets(source);
  collectVisibleLiterals(file, source, lineStarts, literalFindings);
  if (
    source.includes("react-i18next") ||
    source.includes('from "@/lib/i18n"') ||
    source.includes("from '@/lib/i18n'")
  ) {
    collectTranslationCalls(file, source, translationCalls);
  }
}

const missingAll = [];
const missingEnglish = [];
const incompleteRequired = [];
const partial = [];
for (const call of translationCalls.values()) {
  const missing = LANGUAGES.filter(
    (language) => !localeKeys[language].has(call.key),
  );
  if (missing.length === 0) continue;
  const finding = { ...call, missing };
  partial.push(finding);
  if (missing.length === LANGUAGES.length) missingAll.push(finding);
  if (missing.includes("en")) missingEnglish.push(finding);
  if (
    REQUIRED_COMPLETE_KEYS.has(call.key) ||
    REQUIRED_COMPLETE_PREFIXES.some((prefix) => call.key.startsWith(prefix))
  ) {
    incompleteRequired.push(finding);
  }
}

const byFile = new Map();
for (const finding of literalFindings) {
  const relative = path.relative(ROOT, finding.file);
  byFile.set(relative, (byFile.get(relative) || 0) + 1);
}

console.log(
  `web-i18n-audit visible_literals=${literalFindings.length}/${MAX_TOTAL} ` +
    `static_keys=${translationCalls.size} partial_keys=${partial.length}/${MAX_PARTIAL_KEYS} ` +
    `missing_all=${missingAll.length} missing_en=${missingEnglish.length} ` +
    `required_incomplete=${incompleteRequired.length}`,
);
if (byFile.size > 0) {
  console.log(
    `web-i18n-visible-files ${[...byFile.entries()]
      .map(([file, count]) => `${file}:${count}`)
      .join(", ")}`,
  );
}

let failed = false;
if (literalFindings.length > MAX_TOTAL) {
  failed = true;
  printFindings("visible literal", literalFindings);
}
if (partial.length > MAX_PARTIAL_KEYS) {
  failed = true;
  printFindings("partial locale key", partial);
}
if (missingAll.length > 0) {
  failed = true;
  printFindings("key missing from every locale", missingAll);
}
if (missingEnglish.length > 0) {
  failed = true;
  printFindings("key missing from English fallback", missingEnglish);
}
if (incompleteRequired.length > 0) {
  failed = true;
  printFindings("required four-language key", incompleteRequired);
}
if (failed) {
  throw new Error("web i18n audit failed");
}

function walk(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const target = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(target) : [target];
  });
}

function mergeTrees(base, overlay) {
  const merged = { ...base };
  for (const [key, value] of Object.entries(overlay)) {
    const existing = merged[key];
    merged[key] =
      isObject(existing) && isObject(value)
        ? mergeTrees(existing, value)
        : value;
  }
  return merged;
}

function isObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function flattenKeys(tree, prefix = "", output = new Set()) {
  for (const [key, value] of Object.entries(tree)) {
    const qualified = prefix ? `${prefix}.${key}` : key;
    if (isObject(value)) flattenKeys(value, qualified, output);
    else output.add(qualified);
  }
  return output;
}

function loadLocaleKeys() {
  const inlineServerKeys = loadInlineServerResourceKeys();
  return Object.fromEntries(
    LANGUAGES.map((language) => {
      const base = JSON.parse(
        fs.readFileSync(
          path.join(WEB_SRC, "i18n", "locales", `${language}.json`),
          "utf8",
        ),
      );
      const server = JSON.parse(
        fs.readFileSync(
          path.join(WEB_SRC, "i18n", "server-locales", `${language}.json`),
          "utf8",
        ),
      );
      const keys = flattenKeys(mergeTrees(base, server));
      for (const key of inlineServerKeys[language]) {
        keys.add(key);
      }
      for (const namespace of ACCOUNT_TRANSLATION_NAMESPACES) {
        for (const key of ACCOUNT_TRANSLATION_KEYS) {
          keys.add(`${namespace}.${key}`);
        }
      }
      return [language, keys];
    }),
  );
}

function loadInlineServerResourceKeys() {
  const keys = Object.fromEntries(
    LANGUAGES.map((language) => [language, new Set()]),
  );
  const file = path.join(WEB_SRC, "lib", "i18n.tsx");
  const source = fs.readFileSync(file, "utf8");
  const sourceFile = ts.createSourceFile(
    file,
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX,
  );

  const visit = (node) => {
    if (
      ts.isVariableDeclaration(node) &&
      ts.isIdentifier(node.name) &&
      node.name.text === "serverResources" &&
      node.initializer &&
      ts.isObjectLiteralExpression(node.initializer)
    ) {
      for (const property of node.initializer.properties) {
        if (!ts.isPropertyAssignment(property)) continue;
        const language = propertyName(property.name);
        if (
          !language ||
          !LANGUAGES.includes(language) ||
          !ts.isObjectLiteralExpression(property.initializer)
        ) {
          continue;
        }
        collectObjectLiteralKeys(property.initializer, "", keys[language]);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
  return keys;
}

function collectObjectLiteralKeys(node, prefix, output) {
  for (const property of node.properties) {
    if (!ts.isPropertyAssignment(property)) continue;
    const name = propertyName(property.name);
    if (!name) continue;
    const qualified = prefix ? `${prefix}.${name}` : name;
    if (ts.isObjectLiteralExpression(property.initializer)) {
      collectObjectLiteralKeys(property.initializer, qualified, output);
    } else {
      output.add(qualified);
    }
  }
}

function propertyName(node) {
  if (
    ts.isIdentifier(node) ||
    ts.isStringLiteralLike(node) ||
    ts.isNumericLiteral(node)
  ) {
    return node.text;
  }
  return null;
}

function collectVisibleLiterals(file, source, lineStarts, output) {
  const patterns = [
    {
      kind: "jsx-text",
      expression: />\s*([A-Z][A-Za-z][A-Za-z0-9 ,:;()_./%+&'’?-]{2,})\s*</g,
      valueIndex: 1,
    },
    {
      kind: "jsx-text-cjk",
      expression: />\s*([^<>{}\r\n]*[一-龥][^<>{}\r\n]*)\s*</g,
      valueIndex: 1,
    },
  ];
  for (const pattern of patterns) {
    for (const match of source.matchAll(pattern.expression)) {
      const value = match[pattern.valueIndex].trim();
      if (shouldIgnoreText(value)) continue;
      output.push({
        file,
        line: lineOf(lineStarts, match.index),
        value,
        kind: pattern.kind,
      });
    }
  }
  for (const match of source.matchAll(
    /\b(label|title|subtitle|placeholder|aria-label)=["']([^"'{}]{3,})["']/g,
  )) {
    const [, prop, value] = match;
    if (!/[A-Za-z一-龥]/.test(value) || shouldIgnoreText(value)) continue;
    const component = componentNameBefore(source, match.index);
    if (
      component &&
      ignoredPropNames.has(prop) &&
      ignoredUppercasePropComponents.has(component)
    ) {
      continue;
    }
    output.push({
      file,
      line: lineOf(lineStarts, match.index),
      value,
      kind: `${prop}-prop`,
    });
  }
}

function collectTranslationCalls(file, source, output) {
  const sourceFile = ts.createSourceFile(
    file,
    source,
    ts.ScriptTarget.Latest,
    true,
    file.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
  const visit = (node) => {
    if (
      ts.isCallExpression(node) &&
      ts.isIdentifier(node.expression) &&
      node.expression.text === "t" &&
      node.arguments.length > 0 &&
      ts.isStringLiteralLike(node.arguments[0])
    ) {
      const key = node.arguments[0].text;
      if (!output.has(key)) {
        output.set(key, {
          key,
          file,
          line:
            sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile))
              .line + 1,
        });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
}

function printFindings(label, findings) {
  for (const finding of findings.slice(0, 80)) {
    const detail = finding.key
      ? `${JSON.stringify(finding.key)} missing=${finding.missing.join(",")}`
      : `${finding.kind}: ${JSON.stringify(finding.value)}`;
    console.error(
      `${label}: ${path.relative(ROOT, finding.file)}:${finding.line}: ${detail}`,
    );
  }
}

function lineStartOffsets(source) {
  const offsets = [0];
  for (let index = 0; index < source.length; index += 1) {
    if (source[index] === "\n") offsets.push(index + 1);
  }
  return offsets;
}

function shouldIgnoreText(value) {
  if (ignoredText.has(value)) return true;
  if (/^[A-Z0-9_./ -]+$/.test(value) && value.length <= 20) return true;
  if (
    /^(Claude|Codex|Gemini|Kiro|Copilot|GitHub|AWS|OAuth|OpenRouter|Ollama)(?:\b|$)/.test(
      value,
    )
  ) {
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
