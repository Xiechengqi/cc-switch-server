#!/usr/bin/env node
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const requireFromWeb = createRequire(path.join(repoRoot, "web-src/package.json"));
const ts = requireFromWeb("typescript");
const outputPath = path.join(
  repoRoot,
  "assets/contract/upstream-provider-source-baseline.json",
);
const pinnedBaseline = JSON.parse(fs.readFileSync(outputPath, "utf8"));
const upstreamRoot =
  process.env.CC_SWITCH_PROVIDER_AUDIT_ROOT || pinnedBaseline.upstream.repository;
let upstreamCommit = pinnedBaseline.upstream.commit;
const serverInventoryPath = path.join(
  repoRoot,
  "assets/contract/server-provider-legacy-inventory.json",
);
const checkMode = process.argv.includes("--check");
const refreshSource = process.argv.includes("--refresh-source");

const expectedCounts = Object.freeze({
  upstreamProviderTypes: 16,
  serverProviderTypes: 20,
  appPresets: Object.freeze({ claude: 15, codex: 7, gemini: 4 }),
  universalRecipes: 2,
  serverPresets: Object.freeze({ claude: 16, codex: 8, gemini: 5 }),
});

const sourceFiles = {
  providerTypes: "src-tauri/src/proxy/providers/mod.rs",
  claude: "src/config/claudeProviderPresets.ts",
  codex: "src/config/codexProviderPresets.ts",
  gemini: "src/config/geminiProviderPresets.ts",
  universal: "src/config/universalProviderPresets.ts",
};

function git(args, { buffer = false } = {}) {
  return execFileSync("git", ["-C", upstreamRoot, ...args], {
    encoding: buffer ? null : "utf8",
    maxBuffer: 32 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function readPinned(relativePath) {
  return git(["show", `${upstreamCommit}:${relativePath}`], { buffer: true });
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

export function rejectConflictMarkers(relativePath, source) {
  if (
    source.includes("<<<<<<< ") ||
    source.includes("\n=======\n") ||
    source.includes("\n>>>>>>> ")
  ) {
    throw new Error(`pinned upstream source contains conflict marker: ${relativePath}`);
  }
}

function collectConstants(sourceFile) {
  const constants = new Map();
  for (const statement of sourceFile.statements) {
    if (!ts.isVariableStatement(statement)) continue;
    for (const declaration of statement.declarationList.declarations) {
      if (ts.isIdentifier(declaration.name) && declaration.initializer) {
        constants.set(declaration.name.text, declaration.initializer);
      }
    }
  }
  return constants;
}

function propertyName(node) {
  if (ts.isIdentifier(node) || ts.isStringLiteral(node) || ts.isNumericLiteral(node)) {
    return node.text;
  }
  throw new Error(`unsupported computed property at ${node.getSourceFile().fileName}:${node.pos}`);
}

function staticEvaluator(sourceFile) {
  const constants = collectConstants(sourceFile);
  const resolving = new Set();

  function evaluate(node) {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
      return node.text;
    }
    if (ts.isNumericLiteral(node)) return Number(node.text);
    if (node.kind === ts.SyntaxKind.TrueKeyword) return true;
    if (node.kind === ts.SyntaxKind.FalseKeyword) return false;
    if (node.kind === ts.SyntaxKind.NullKeyword) return null;
    if (
      ts.isAsExpression(node) ||
      ts.isTypeAssertionExpression(node) ||
      ts.isParenthesizedExpression(node) ||
      ts.isNonNullExpression(node) ||
      (typeof ts.isSatisfiesExpression === "function" && ts.isSatisfiesExpression(node))
    ) {
      return evaluate(node.expression);
    }
    if (ts.isPrefixUnaryExpression(node) && node.operator === ts.SyntaxKind.MinusToken) {
      return -Number(evaluate(node.operand));
    }
    if (ts.isArrayLiteralExpression(node)) {
      return node.elements.map((element) => evaluate(element));
    }
    if (ts.isObjectLiteralExpression(node)) {
      const output = {};
      for (const property of node.properties) {
        if (ts.isPropertyAssignment(property)) {
          output[propertyName(property.name)] = evaluate(property.initializer);
        } else if (ts.isShorthandPropertyAssignment(property)) {
          output[property.name.text] = evaluateIdentifier(property.name.text);
        } else if (ts.isSpreadAssignment(property)) {
          Object.assign(output, evaluate(property.expression));
        } else {
          throw new Error(
            `unsupported object member at ${sourceFile.fileName}:${property.pos}`,
          );
        }
      }
      return output;
    }
    if (ts.isIdentifier(node)) return evaluateIdentifier(node.text);
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)) {
      const name = node.expression.text;
      const args = node.arguments.map((argument) => evaluate(argument));
      if (name === "generateThirdPartyAuth") {
        return { OPENAI_API_KEY: args[0] ?? "" };
      }
      if (name === "generateThirdPartyConfig") {
        return {
          $staticCall: name,
          providerName: args[0],
          baseUrl: args[1],
          model: args[2] ?? "gpt-5.5",
        };
      }
      if (name === "modelCatalog" || name === "deepClone") return args[0];
      throw new Error(`unsupported call ${name} at ${sourceFile.fileName}:${node.pos}`);
    }
    throw new Error(
      `unsupported AST node ${ts.SyntaxKind[node.kind]} at ${sourceFile.fileName}:${node.pos}`,
    );
  }

  function evaluateIdentifier(name) {
    if (!constants.has(name)) {
      throw new Error(`unresolved identifier ${name} in ${sourceFile.fileName}`);
    }
    if (resolving.has(name)) {
      throw new Error(`cyclic constant ${name} in ${sourceFile.fileName}`);
    }
    resolving.add(name);
    try {
      return evaluate(constants.get(name));
    } finally {
      resolving.delete(name);
    }
  }

  return { evaluateIdentifier };
}

function nested(value, ...keys) {
  let current = value;
  for (const key of keys) {
    if (!current || typeof current !== "object") return undefined;
    current = current[key];
  }
  return current;
}

function firstString(...values) {
  return values.find((value) => typeof value === "string" && value.length > 0) ?? null;
}

function collectPointers(value, prefix = "") {
  if (!value || typeof value !== "object") return [];
  const pointers = [];
  for (const [key, child] of Object.entries(value)) {
    const pointer = `${prefix}/${key.replaceAll("~", "~0").replaceAll("/", "~1")}`;
    pointers.push(pointer);
    pointers.push(...collectPointers(child, pointer));
  }
  return pointers.sort();
}

function normalizePreset(app, preset, sourceIndex) {
  const configCall = preset.config?.$staticCall === "generateThirdPartyConfig"
    ? preset.config
    : null;
  const env = nested(preset, "settingsConfig", "env") ?? {};
  return {
    sourceIndex,
    name: preset.name,
    providerType: preset.providerType ?? null,
    apiFormat: preset.apiFormat ?? null,
    category: preset.category ?? null,
    official: preset.isOfficial === true,
    requiresOAuth: preset.requiresOAuth === true,
    websiteUrl: preset.websiteUrl ?? null,
    baseUrl: firstString(
      preset.baseURL,
      env.ANTHROPIC_BASE_URL,
      env.OPENAI_BASE_URL,
      env.GOOGLE_GEMINI_BASE_URL,
      env.GEMINI_BASE_URL,
      configCall?.baseUrl,
    ),
    defaultModel: firstString(
      preset.model,
      nested(preset, "settingsConfig", "modelMapping", "upstreamModel"),
      env.ANTHROPIC_MODEL,
      env.OPENAI_MODEL,
      env.GEMINI_MODEL,
      configCall?.model,
    ),
    declaredPointers: collectPointers(preset),
    sourceApp: app,
  };
}

function extractPresetFile(relativePath, variableName, app) {
  const sourceBuffer = readPinned(relativePath);
  return extractPresetSource(relativePath, sourceBuffer, variableName, app);
}

export function extractPresetSource(relativePath, sourceBuffer, variableName, app) {
  const source = sourceBuffer.toString("utf8");
  rejectConflictMarkers(relativePath, source);
  const sourceFile = ts.createSourceFile(
    relativePath,
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  const parseErrors = sourceFile.parseDiagnostics ?? [];
  if (parseErrors.length > 0) {
    throw new Error(`TypeScript parse failed for ${relativePath}`);
  }
  const evaluator = staticEvaluator(sourceFile);
  const raw = evaluator.evaluateIdentifier(variableName);
  if (!Array.isArray(raw) || raw.length === 0) {
    throw new Error(`no presets extracted from ${relativePath}:${variableName}`);
  }
  return raw.map((preset, index) => normalizePreset(app, preset, index));
}

function extractServerPresetFile(relativePath, variableName, app) {
  const absolutePath = path.join(repoRoot, relativePath);
  return extractPresetSource(
    relativePath,
    fs.readFileSync(absolutePath),
    variableName,
    app,
  );
}

function extractUniversal(relativePath) {
  const sourceBuffer = readPinned(relativePath);
  const source = sourceBuffer.toString("utf8");
  rejectConflictMarkers(relativePath, source);
  const sourceFile = ts.createSourceFile(
    relativePath,
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  const evaluator = staticEvaluator(sourceFile);
  const raw = evaluator.evaluateIdentifier("universalProviderPresets");
  return raw.map((preset, sourceIndex) => ({
    sourceIndex,
    name: preset.name,
    providerType: preset.providerType,
    defaultApps: preset.defaultApps,
    defaultModels: preset.defaultModels,
    customTemplate: preset.isCustomTemplate === true,
    websiteUrl: preset.websiteUrl ?? null,
    declaredPointers: collectPointers(preset),
  }));
}

function extractProviderTypes(relativePath) {
  return extractProviderTypesSource(
    relativePath,
    readPinned(relativePath).toString("utf8"),
  );
}

export function extractProviderTypesSource(relativePath, source) {
  rejectConflictMarkers(relativePath, source);
  const enumMatch = /pub enum ProviderType\s*\{([\s\S]*?)\n\}/.exec(source);
  const enumBody = enumMatch?.[1];
  const afterEnum = enumMatch
    ? source.slice(enumMatch.index + enumMatch[0].length)
    : "";
  const asStrBody = afterEnum.match(
    /pub fn as_str\((?:&)?self\)\s*->\s*&'static str\s*\{([\s\S]*?)\n\s*\}/,
  )?.[1];
  if (!enumBody || !asStrBody) {
    throw new Error(`unable to parse ProviderType contract from ${relativePath}`);
  }
  const unparsedEnumBody = enumBody
    .replace(/^\s*\/\/\/?.*$/gm, "")
    .replace(/^\s*#\[[^\]]+\]\s*$/gm, "")
    .replace(/^\s*[A-Z][A-Za-z0-9]*\s*,\s*$/gm, "")
    .trim();
  if (unparsedEnumBody) {
    throw new Error(
      `unsupported ProviderType enum syntax in ${relativePath}: ${unparsedEnumBody.split("\n")[0]}`,
    );
  }
  const variants = [...enumBody.matchAll(/^\s*([A-Z][A-Za-z0-9]*)\s*,/gm)].map(
    (match) => match[1],
  );
  if (variants.length === 0 || new Set(variants).size !== variants.length) {
    throw new Error(`ProviderType variants are empty or duplicated in ${relativePath}`);
  }
  const ids = new Map(
    [...asStrBody.matchAll(/(?:ProviderType|Self)::([A-Za-z0-9]+)\s*=>\s*"([^"]+)"/g)].map(
      (match) => [match[1], match[2]],
    ),
  );
  const missing = variants.filter((variant) => !ids.has(variant));
  if (missing.length > 0) {
    throw new Error(`ProviderType variants without as_str: ${missing.join(", ")}`);
  }
  const extra = [...ids.keys()].filter((variant) => !variants.includes(variant));
  if (extra.length > 0) {
    throw new Error(`ProviderType as_str arms without variants: ${extra.join(", ")}`);
  }
  const values = variants.map((variant) => ids.get(variant));
  if (new Set(values).size !== values.length) {
    throw new Error(`ProviderType as_str ids are duplicated in ${relativePath}`);
  }
  return variants.map((variant) => ({ variant, id: ids.get(variant) }));
}

export function extractServerProviderTypesSource(relativePath, source) {
  const providerTypes = extractProviderTypesSource(relativePath, source);
  const enumBody = source.match(/pub enum ProviderType\s*\{([\s\S]*?)\n\}/)?.[1] ?? "";
  const serdeIds = new Map(
    [...enumBody.matchAll(/#\[serde\(rename\s*=\s*"([^"]+)"\)\]\s*([A-Z][A-Za-z0-9]*)\s*,/g)].map(
      (match) => [match[2], match[1]],
    ),
  );
  if (serdeIds.size !== providerTypes.length) {
    throw new Error(`Server ProviderType serde mappings are incomplete in ${relativePath}`);
  }
  for (const providerType of providerTypes) {
    if (serdeIds.get(providerType.variant) !== providerType.id) {
      throw new Error(
        `Server ProviderType serde/as_str mismatch for ${providerType.variant} in ${relativePath}`,
      );
    }
  }
  return providerTypes;
}

function buildBaseline() {
  const resolvedCommit = git(["rev-parse", upstreamCommit]).trim();
  if (resolvedCommit !== upstreamCommit) {
    throw new Error("upstream provider baseline must pin a full commit");
  }
  const sources = Object.values(sourceFiles).map((relativePath) => {
    const content = readPinned(relativePath);
    rejectConflictMarkers(relativePath, content.toString("utf8"));
    return { path: relativePath, sha256: sha256(content) };
  });
  const appPresets = {
    claude: extractPresetFile(
      sourceFiles.claude,
      "providerPresets",
      "claude",
    ),
    codex: extractPresetFile(
      sourceFiles.codex,
      "codexProviderPresets",
      "codex",
    ),
    gemini: extractPresetFile(
      sourceFiles.gemini,
      "geminiProviderPresets",
      "gemini",
    ),
  };
  const universalRecipes = extractUniversal(sourceFiles.universal);
  return {
    schemaVersion: 1,
    upstream: {
      repository: pinnedBaseline.upstream.repository,
      commit: upstreamCommit,
    },
    sources,
    providerTypes: extractProviderTypes(sourceFiles.providerTypes),
    appPresets,
    universalRecipes,
    counts: {
      providerTypes: extractProviderTypes(sourceFiles.providerTypes).length,
      appPresets: Object.fromEntries(
        Object.entries(appPresets).map(([app, presets]) => [app, presets.length]),
      ),
      universalRecipes: universalRecipes.length,
    },
  };
}

function buildServerLegacyInventory() {
  const files = {
    claude: "web-src/src/config/claudeProviderPresets.ts",
    codex: "web-src/src/config/codexProviderPresets.ts",
    gemini: "web-src/src/config/geminiProviderPresets.ts",
  };
  const presets = {
    claude: extractServerPresetFile(files.claude, "providerPresets", "claude"),
    codex: extractServerPresetFile(files.codex, "codexProviderPresets", "codex"),
    gemini: extractServerPresetFile(files.gemini, "geminiProviderPresets", "gemini"),
  };
  const providerTypePath = "src/domain/providers/model.rs";
  const providerTypeSource = fs.readFileSync(path.join(repoRoot, providerTypePath));
  return {
    schemaVersion: 1,
    authority: "migration-input-only",
    note: "This freezes the legacy Web preset inventory. Phase 2 replaces it with the Rust ProfileSpec registry.",
    providerTypeSource: {
      path: providerTypePath,
      sha256: sha256(providerTypeSource),
    },
    providerTypes: extractServerProviderTypesSource(
      providerTypePath,
      providerTypeSource.toString("utf8"),
    ),
    sources: Object.entries(files).map(([app, relativePath]) => ({
      app,
      path: relativePath,
      sha256: sha256(fs.readFileSync(path.join(repoRoot, relativePath))),
    })),
    presets,
    counts: {
      providerTypes: extractServerProviderTypesSource(
        providerTypePath,
        providerTypeSource.toString("utf8"),
      ).length,
      presets: Object.fromEntries(
        Object.entries(presets).map(([app, values]) => [app, values.length]),
      ),
    },
  };
}

function buildCoverageMappings(baseline, serverInventory) {
  const serverTypes = new Set(serverInventory.providerTypes.map((entry) => entry.id));
  const providerTypes = baseline.providerTypes.map((entry) => {
    if (!serverTypes.has(entry.id)) {
      throw new Error(`upstream ProviderType ${entry.id} has no Server mapping`);
    }
    return { upstreamId: entry.id, serverId: entry.id };
  });
  const appPresets = {};
  const serverOnlyPresets = {};
  for (const app of ["claude", "codex", "gemini"]) {
    const serverByName = new Map(
      serverInventory.presets[app].map((preset) => [preset.name, preset]),
    );
    appPresets[app] = baseline.appPresets[app].map((preset) => {
      const target = serverByName.get(preset.name);
      if (!target) {
        throw new Error(`upstream preset ${app}/${preset.name} has no Server mapping`);
      }
      return {
        upstreamSourceIndex: preset.sourceIndex,
        upstreamName: preset.name,
        serverSourceIndex: target.sourceIndex,
        serverName: target.name,
      };
    });
    const upstreamNames = new Set(baseline.appPresets[app].map((preset) => preset.name));
    serverOnlyPresets[app] = serverInventory.presets[app]
      .filter((preset) => !upstreamNames.has(preset.name))
      .map((preset) => ({ serverSourceIndex: preset.sourceIndex, serverName: preset.name }));
  }
  return {
    providerTypes,
    appPresets,
    serverOnlyPresets,
    universalRecipes: baseline.universalRecipes.map((recipe) => ({
      upstreamProviderType: recipe.providerType,
      targetProfiles: {
        claude: "claude.custom_http",
        codex: "codex.custom_http",
        gemini: "gemini.custom_http",
      },
    })),
    plannedCompatibilityProfiles: [
      "claude.legacy_compat",
      "codex.legacy_compat",
      "gemini.legacy_compat",
    ],
    firstClassProfileAdditions: [
      "claude.anthropic_api_key",
      "codex.openai_api_key",
      "gemini.google_api_key",
    ],
  };
}

function assertExactKeys(value, expected, label) {
  const actual = Object.keys(value ?? {}).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    throw new Error(`${label} keys must be ${wanted.join(", ")}; got ${actual.join(", ")}`);
  }
}

function assertPresetInventory(presetsByApp, expectedByApp, label) {
  assertExactKeys(presetsByApp, ["claude", "codex", "gemini"], label);
  for (const [app, expectedCount] of Object.entries(expectedByApp)) {
    const presets = presetsByApp[app];
    if (!Array.isArray(presets) || presets.length !== expectedCount) {
      throw new Error(
        `${label}.${app} requires reviewed count ${expectedCount}; got ${presets?.length ?? "missing"}`,
      );
    }
    const names = new Set();
    presets.forEach((preset, sourceIndex) => {
      if (!preset || typeof preset.name !== "string" || !preset.name.trim()) {
        throw new Error(`${label}.${app}[${sourceIndex}] is missing a name`);
      }
      if (preset.sourceIndex !== sourceIndex || preset.sourceApp !== app) {
        throw new Error(`${label}.${app}[${sourceIndex}] has an invalid source identity`);
      }
      const normalizedName = preset.name.trim().toLocaleLowerCase("en-US");
      if (names.has(normalizedName)) {
        throw new Error(`${label}.${app} contains duplicate preset name ${preset.name}`);
      }
      names.add(normalizedName);
      if (!Array.isArray(preset.declaredPointers) || !preset.declaredPointers.includes("/name")) {
        throw new Error(`${label}.${app}[${sourceIndex}] has incomplete field evidence`);
      }
    });
  }
}

export function validateBaselineContracts(baseline, serverInventory) {
  if (baseline?.schemaVersion !== 1 || serverInventory?.schemaVersion !== 1) {
    throw new Error("provider source baseline schemaVersion must be 1");
  }
  if (serverInventory.authority !== "migration-input-only") {
    throw new Error("server preset inventory must remain migration-input-only");
  }
  if (!/^[0-9a-f]{40}$/.test(baseline.upstream?.commit ?? "")) {
    throw new Error("provider source baseline must pin a full upstream commit");
  }
  if (!Array.isArray(baseline.sources) || baseline.sources.length !== 5) {
    throw new Error("provider source baseline must contain exactly five authoritative sources");
  }
  const sourcePaths = new Set(baseline.sources.map((source) => source.path));
  if (sourcePaths.size !== baseline.sources.length) {
    throw new Error("provider source baseline contains duplicate source paths");
  }
  for (const source of baseline.sources) {
    if (!/^[0-9a-f]{64}$/.test(source.sha256 ?? "")) {
      throw new Error(`provider source baseline has an invalid hash for ${source.path}`);
    }
  }
  if (
    !Array.isArray(baseline.providerTypes) ||
    baseline.providerTypes.length !== expectedCounts.upstreamProviderTypes
  ) {
    throw new Error(
      `upstream ProviderType inventory requires reviewed count ${expectedCounts.upstreamProviderTypes}; got ${baseline.providerTypes?.length ?? "missing"}`,
    );
  }
  const variants = new Set(baseline.providerTypes.map((entry) => entry.variant));
  const ids = new Set(baseline.providerTypes.map((entry) => entry.id));
  if (variants.size !== baseline.providerTypes.length || ids.size !== baseline.providerTypes.length) {
    throw new Error("ProviderType variants and ids must both be unique");
  }
  assertPresetInventory(baseline.appPresets, expectedCounts.appPresets, "upstream presets");
  assertPresetInventory(serverInventory.presets, expectedCounts.serverPresets, "server presets");
  if (
    !Array.isArray(serverInventory.providerTypes) ||
    serverInventory.providerTypes.length !== expectedCounts.serverProviderTypes
  ) {
    throw new Error(
      `Server ProviderType inventory requires reviewed count ${expectedCounts.serverProviderTypes}; got ${serverInventory.providerTypes?.length ?? "missing"}`,
    );
  }
  if (!/^[0-9a-f]{64}$/.test(serverInventory.providerTypeSource?.sha256 ?? "")) {
    throw new Error("Server ProviderType source requires a content hash");
  }
  const serverVariants = new Set(serverInventory.providerTypes.map((entry) => entry.variant));
  const serverIds = new Set(serverInventory.providerTypes.map((entry) => entry.id));
  if (
    serverVariants.size !== serverInventory.providerTypes.length ||
    serverIds.size !== serverInventory.providerTypes.length
  ) {
    throw new Error("Server ProviderType variants and ids must both be unique");
  }
  if (
    !Array.isArray(baseline.universalRecipes) ||
    baseline.universalRecipes.length !== expectedCounts.universalRecipes
  ) {
    throw new Error(
      `Universal inventory requires reviewed count ${expectedCounts.universalRecipes}; got ${baseline.universalRecipes?.length ?? "missing"}`,
    );
  }
  const universalTypes = new Set();
  baseline.universalRecipes.forEach((recipe, sourceIndex) => {
    if (
      recipe.sourceIndex !== sourceIndex ||
      typeof recipe.providerType !== "string" ||
      !recipe.providerType
    ) {
      throw new Error(`Universal recipe ${sourceIndex} has an invalid source identity`);
    }
    if (universalTypes.has(recipe.providerType)) {
      throw new Error(`Universal recipe providerType is duplicated: ${recipe.providerType}`);
    }
    universalTypes.add(recipe.providerType);
    assertExactKeys(recipe.defaultApps, ["claude", "codex", "gemini"], "Universal apps");
  });
  const actualCounts = {
    providerTypes: baseline.providerTypes.length,
    appPresets: Object.fromEntries(
      Object.entries(baseline.appPresets).map(([app, presets]) => [app, presets.length]),
    ),
    universalRecipes: baseline.universalRecipes.length,
  };
  const actualServerCounts = {
    providerTypes: serverInventory.providerTypes.length,
    presets: Object.fromEntries(
      Object.entries(serverInventory.presets).map(([app, presets]) => [app, presets.length]),
    ),
  };
  if (JSON.stringify(baseline.counts) !== JSON.stringify(actualCounts)) {
    throw new Error("provider source baseline counts do not match extracted records");
  }
  if (JSON.stringify(serverInventory.counts) !== JSON.stringify(actualServerCounts)) {
    throw new Error("server preset inventory counts do not match extracted records");
  }
  const mappings = serverInventory.coverageMappings;
  if (!mappings || mappings.providerTypes.length !== baseline.providerTypes.length) {
    throw new Error("provider coverage mappings are missing ProviderType entries");
  }
  for (const entry of mappings.providerTypes) {
    if (!ids.has(entry.upstreamId) || !serverIds.has(entry.serverId)) {
      throw new Error(`invalid ProviderType coverage mapping ${entry.upstreamId}`);
    }
  }
  for (const app of ["claude", "codex", "gemini"]) {
    if (mappings.appPresets?.[app]?.length !== baseline.appPresets[app].length) {
      throw new Error(`provider coverage mappings are incomplete for ${app}`);
    }
    if (
      mappings.serverOnlyPresets?.[app]?.length !==
      serverInventory.presets[app].length - baseline.appPresets[app].length
    ) {
      throw new Error(`server-only preset mappings are incomplete for ${app}`);
    }
  }
  if (mappings.universalRecipes?.length !== baseline.universalRecipes.length) {
    throw new Error("Universal recipe coverage mappings are incomplete");
  }
  if (
    !Array.isArray(mappings.firstClassProfileAdditions) ||
    mappings.firstClassProfileAdditions.length !== 3 ||
    "directApiCandidates" in mappings
  ) {
    throw new Error("three reviewed first-class direct API Profile additions are required");
  }
}

function main() {
  if (checkMode && refreshSource) {
    throw new Error("--check and --refresh-source cannot be used together");
  }
  if (refreshSource) {
    if (git(["status", "--porcelain=v1"]).trim()) {
      throw new Error("refusing to refresh Provider audit baseline from a dirty source tree");
    }
    upstreamCommit = git(["rev-parse", "HEAD"]).trim();
  }
  const baseline = buildBaseline();
  const serverInventory = buildServerLegacyInventory();
  serverInventory.coverageMappings = buildCoverageMappings(baseline, serverInventory);
  validateBaselineContracts(baseline, serverInventory);
  const serialized = `${JSON.stringify(baseline, null, 2)}\n`;
  const serializedServerInventory = `${JSON.stringify(serverInventory, null, 2)}\n`;
  if (checkMode) {
    const current = fs.existsSync(outputPath) ? fs.readFileSync(outputPath, "utf8") : "";
    const currentServerInventory = fs.existsSync(serverInventoryPath)
      ? fs.readFileSync(serverInventoryPath, "utf8")
      : "";
    if (current !== serialized || currentServerInventory !== serializedServerInventory) {
      throw new Error(
        "provider source baseline is out of date; review the pinned commit/server presets and run audit-upstream-provider-baseline.mjs",
      );
    }
    console.log(
      `provider source baseline ok: upstream ${baseline.counts.appPresets.claude}/${baseline.counts.appPresets.codex}/${baseline.counts.appPresets.gemini}, server ${serverInventory.counts.presets.claude}/${serverInventory.counts.presets.codex}/${serverInventory.counts.presets.gemini}, ${serverInventory.counts.providerTypes} server types, ${baseline.counts.universalRecipes} universal`,
    );
    return;
  }
  fs.writeFileSync(outputPath, serialized);
  fs.writeFileSync(serverInventoryPath, serializedServerInventory);
  console.log(
    refreshSource
      ? `Provider audit baseline refreshed at ${upstreamCommit}`
      : `provider source baselines written: ${outputPath}, ${serverInventoryPath}`,
  );
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
