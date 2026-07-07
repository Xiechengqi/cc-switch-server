#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(new URL("../..", import.meta.url).pathname);
const presetSourceRoot =
  process.env.CC_SWITCH_SOURCE_DIR ||
  process.env.CC_SWITCH_UPSTREAM_DIR ||
  "/data/projects/cc-switch";
const providerTypeSourceRoot =
  process.env.CC_SWITCH_PROVIDER_TYPE_SOURCE_DIR || "/data/projects/cc-switch";
const checkMode = process.argv.includes("--check");

const providerTypeSource = path.join(
  providerTypeSourceRoot,
  "src-tauri/src/proxy/providers/mod.rs",
);

const presetFiles = {
  claude: path.join(presetSourceRoot, "src/config/claudeProviderPresets.ts"),
  codex: path.join(presetSourceRoot, "src/config/codexProviderPresets.ts"),
  gemini: path.join(presetSourceRoot, "src/config/geminiProviderPresets.ts"),
  universal: path.join(presetSourceRoot, "src/config/universalProviderPresets.ts"),
};

const requiredProviderTypes = [
  ["claude", "Anthropic official / API key", ["claude"]],
  ["claude_auth", "Claude bearer-only relay", ["claude"]],
  ["claude_oauth", "Claude Official OAuth", ["claude"]],
  ["codex", "OpenAI/Codex compatible", ["codex"]],
  ["codex_oauth", "OpenAI ChatGPT OAuth", ["claude", "codex"]],
  ["gemini", "Google Gemini API key", ["gemini"]],
  ["gemini_cli", "Google Gemini OAuth / CLI", ["gemini", "claude"]],
  ["openrouter", "OpenRouter", ["claude", "codex", "gemini"]],
  ["github_copilot", "GitHub Copilot", ["claude"]],
  ["deepseek_account", "DeepSeek account", ["claude"]],
  ["kiro_oauth", "Kiro OAuth", ["claude"]],
  ["cursor_oauth", "Cursor OAuth", ["claude", "codex"]],
  ["cursor_apikey", "Cursor API key", ["claude", "codex"]],
  ["antigravity_oauth", "Antigravity OAuth", ["claude", "gemini"]],
  ["agy_oauth", "Antigravity CLI / agy", ["claude", "gemini"]],
  ["ollama_cloud", "Ollama API key", ["claude", "codex"]],
];

const serverCompatibilityProviderTypes = [
  ["aws_bedrock", "AWS Bedrock compatibility schema", ["claude"]],
  ["nvidia", "Nvidia OpenAI-compatible API", ["claude", "codex"]],
  ["deepseek_api", "DeepSeek API key", ["claude", "codex"]],
];

function read(file) {
  return fs.readFileSync(file, "utf8");
}

function extractProviderTypeIds() {
  const source = read(providerTypeSource);
  const body = source.match(/pub enum ProviderType \{([\s\S]*?)\n\}/)?.[1] ?? "";
  const variants = [...body.matchAll(/^\s*([A-Z][A-Za-z0-9]*)\s*,/gm)].map(
    (match) => match[1],
  );

  const asStrBody = source.match(/pub fn as_str\(&self\).*?\{([\s\S]*?)\n    \}/)?.[1] ?? "";
  const ids = new Map();
  for (const match of asStrBody.matchAll(/ProviderType::([A-Za-z0-9]+)\s*=>\s*"([^"]+)"/g)) {
    ids.set(match[1], match[2]);
  }

  return variants.map((variant) => ids.get(variant)).filter(Boolean);
}

function extractPresets(file) {
  const source = read(file);
  const arrayStart = source.indexOf("= [");
  const start = source.indexOf("[", arrayStart >= 0 ? arrayStart : 0);
  if (start < 0) return [];

  const presets = [];
  for (const body of topLevelObjects(source.slice(start))) {
    const name = body.match(/^\s*\{\s*name:\s*"([^"]+)"/)?.[1];
    if (!name) continue;
    const providerType = body.match(/providerType:\s*"([^"]+)"/)?.[1] ?? null;
    const apiFormat = body.match(/apiFormat:\s*"([^"]+)"/)?.[1] ?? null;
    const baseUrl = extractBaseUrl(body);
    presets.push({
      name,
      providerType,
      apiFormat,
      baseUrl,
    });
  }
  return dedupePresets(presets);
}

function extractBaseUrl(body) {
  const directBaseUrl = body.match(/baseURL:\s*"([^"]+)"/)?.[1];
  if (directBaseUrl) return directBaseUrl;

  const envBaseUrl = body.match(
    /(?:ANTHROPIC_BASE_URL|GOOGLE_GEMINI_BASE_URL|GEMINI_BASE_URL):\s*"([^"]+)"/,
  )?.[1];
  if (envBaseUrl) return envBaseUrl;

  const codexGeneratedBaseUrl = body.match(
    /generateThirdPartyConfig\(\s*"[^"]+"\s*,\s*"([^"]+)"/,
  )?.[1];
  if (codexGeneratedBaseUrl) return codexGeneratedBaseUrl;

  return null;
}

function topLevelObjects(input) {
  const objects = [];
  let depth = 0;
  let start = -1;
  let inString = false;
  let quote = "";
  let escaped = false;

  for (let i = 0; i < input.length; i += 1) {
    const char = input[i];
    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === quote) {
        inString = false;
      }
      continue;
    }

    if (char === '"' || char === "'" || char === "`") {
      inString = true;
      quote = char;
      continue;
    }

    if (char === "{") {
      if (depth === 0) start = i;
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0 && start >= 0) {
        objects.push(input.slice(start, i + 1));
        start = -1;
      }
    } else if (char === "]" && depth === 0) {
      break;
    }
  }
  return objects;
}

function dedupePresets(items) {
  const seen = new Set();
  const result = [];
  for (const item of items) {
    const key = `${item.name}\u0000${item.providerType ?? ""}`;
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(item);
  }
  return result;
}

function buildCoverage() {
  const sourceProviderTypes = new Set(extractProviderTypeIds());
  const providerTypes = requiredProviderTypes.map(([id, label, apps]) => ({
    id,
    label,
    apps,
    required: true,
    presentInSource: sourceProviderTypes.has(id),
  }));
  providerTypes.push(
    ...serverCompatibilityProviderTypes.map(([id, label, apps]) => ({
      id,
      label,
      apps,
      required: false,
      presentInSource: false,
    })),
  );

  return {
    generatedFrom: {
      providerTypes: providerTypeSourceRoot,
      presets: presetSourceRoot,
    },
    providerTypes,
    presets: {
      claude: extractPresets(presetFiles.claude),
      codex: extractPresets(presetFiles.codex),
      gemini: extractPresets(presetFiles.gemini),
      universal: extractPresets(presetFiles.universal),
    },
  };
}

function providerFixture(app, preset) {
  const settingsConfig = {};
  if (preset.baseUrl) {
    settingsConfig.env = {};
    if (app === "gemini") {
      settingsConfig.env.GOOGLE_GEMINI_BASE_URL = preset.baseUrl;
    } else if (app === "codex") {
      settingsConfig.env.OPENAI_BASE_URL = preset.baseUrl;
    } else if (app === "claude") {
      settingsConfig.env.ANTHROPIC_BASE_URL = preset.baseUrl;
    }
  }

  const meta = {};
  if (preset.providerType) meta.providerType = preset.providerType;
  if (preset.apiFormat) meta.apiFormat = preset.apiFormat;

  return {
    app,
    name: preset.name,
    expectedProviderType: expectedProviderType(app, preset),
    provider: {
      id: `${app}:${preset.name}`,
      name: preset.name,
      settingsConfig,
      meta: Object.keys(meta).length > 0 ? meta : null,
    },
  };
}

function expectedProviderType(app, preset) {
  if (app === "claude") {
    if (preset.providerType === "google_gemini_oauth") return "gemini_cli";
    if (preset.providerType) return preset.providerType;
    if (preset.baseUrl?.includes("openrouter.ai")) return "openrouter";
    if (preset.baseUrl?.includes("bedrock-runtime.")) return "aws_bedrock";
    if (preset.baseUrl?.includes("integrate.api.nvidia.com")) return "nvidia";
    if (preset.baseUrl?.includes("api.deepseek.com")) return "deepseek_api";
    return "claude";
  }

  if (app === "codex") {
    if (
      ["codex_oauth", "cursor_oauth", "cursor_apikey", "ollama_cloud"].includes(
        preset.providerType,
      )
    ) {
      return preset.providerType;
    }
    if (preset.baseUrl?.includes("openrouter.ai")) return "openrouter";
    if (preset.baseUrl?.includes("integrate.api.nvidia.com")) return "nvidia";
    if (preset.baseUrl?.includes("api.deepseek.com")) return "deepseek_api";
    return "codex";
  }

  if (app === "gemini") {
    if (preset.providerType === "google_gemini_oauth") return "gemini_cli";
    if (["antigravity_oauth", "agy_oauth"].includes(preset.providerType)) {
      return preset.providerType;
    }
    if (preset.baseUrl?.includes("openrouter.ai")) return "openrouter";
    return "gemini";
  }

  return null;
}

function toMarkdown(coverage) {
  const lines = [];
  lines.push("# Provider Coverage");
  lines.push("");
  if (typeof coverage.generatedFrom === "string") {
    lines.push(`Generated from: \`${coverage.generatedFrom}\``);
  } else {
    lines.push(`Provider types from: \`${coverage.generatedFrom.providerTypes}\``);
    lines.push(`Presets from: \`${coverage.generatedFrom.presets}\``);
  }
  lines.push("");
  lines.push(
    "Note: server compatibility provider types are explicit cc-switch-server classifications for cc-switch presets that do not carry an upstream `providerType`.",
  );
  lines.push("");
  lines.push("## Provider Types");
  lines.push("");
  lines.push("| ProviderType | Apps | Required | Present in source |");
  lines.push("| --- | --- | --- | --- |");
  for (const item of coverage.providerTypes) {
    lines.push(
      `| \`${item.id}\` | ${item.apps.join(", ")} | ${item.required ? "yes" : "no"} | ${item.presentInSource ? "yes" : "NO"} |`,
    );
  }
  lines.push("");
  for (const key of ["claude", "codex", "gemini", "universal"]) {
    lines.push(`## ${key} presets`);
    lines.push("");
    lines.push("| Name | providerType |");
    lines.push("| --- | --- |");
    for (const preset of coverage.presets[key]) {
      lines.push(`| ${preset.name} | ${preset.providerType ? `\`${preset.providerType}\`` : ""} |`);
    }
    lines.push("");
  }
  return `${lines.join("\n").trimEnd()}\n`;
}

function assertCoverage(coverage) {
  const missingTypes = coverage.providerTypes
    .filter((item) => item.required && !item.presentInSource)
    .map((item) => item.id);
  if (missingTypes.length > 0) {
    throw new Error(`Missing provider types in source: ${missingTypes.join(", ")}`);
  }
  for (const key of ["claude", "codex", "gemini"]) {
    if (coverage.presets[key].length === 0) {
      throw new Error(`No ${key} presets extracted`);
    }
  }
}

function writeIfChanged(file, content) {
  const existing = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null;
  if (existing === content) return false;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content);
  return true;
}

const coverage = buildCoverage();
assertCoverage(coverage);
coverage.fixtures = {
  claude: coverage.presets.claude.map((preset) => providerFixture("claude", preset)),
  codex: coverage.presets.codex.map((preset) => providerFixture("codex", preset)),
  gemini: coverage.presets.gemini.map((preset) => providerFixture("gemini", preset)),
};

const jsonPath = path.join(repoRoot, "assets/contract/provider-coverage.json");
const mdPath = path.join(repoRoot, "docs/provider-coverage.md");
const json = `${JSON.stringify(coverage, null, 2)}\n`;
const markdown = toMarkdown(coverage);

if (checkMode) {
  const actualJson = fs.existsSync(jsonPath) ? fs.readFileSync(jsonPath, "utf8") : "";
  const actualMd = fs.existsSync(mdPath) ? fs.readFileSync(mdPath, "utf8") : "";
  if (actualJson !== json || actualMd !== markdown) {
    throw new Error("provider coverage assets/docs are out of date; run scripts/audit/audit-provider-coverage.mjs");
  }
  console.log("provider coverage assets/docs are up to date");
} else {
  const changed =
    writeIfChanged(jsonPath, json) | writeIfChanged(mdPath, markdown);
  console.log(changed ? "provider coverage assets/docs updated" : "provider coverage assets/docs unchanged");
}
