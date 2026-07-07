#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';

const upstreamRoot = process.env.CC_SWITCH_UPSTREAM || '/data/projects/cc-switch';
const outputDir = join(process.cwd(), 'assets/contract/provider-fixtures');
const sourceDirs = [
  'src-tauri/src/proxy/providers',
  'src-tauri/src/proxy',
  'src',
  'web',
];
const interestingNames = [
  'claudeProviderPresets',
  'codexProviderPresets',
  'geminiProviderPresets',
  'ProviderType',
  'modelMapping',
  'authBinding',
  'settingsConfig',
  'settings_config',
  'testConfig',
  'codex',
  'gemini',
];
const structuralFields = [
  ['settingsConfig', ['settingsConfig', 'settings_config']],
  ['meta', ['meta', 'ProviderMeta']],
  ['models', ['models', 'modelMapping']],
  ['modelMapping', ['modelMapping']],
  ['testConfig', ['testConfig', 'test_config']],
  ['authBinding', ['authBinding', 'auth_binding']],
  ['codexConfig', ['codex', 'Codex', 'OPENAI_BASE_URL', 'CODEX_BASE_URL']],
  ['geminiConfig', ['gemini', 'Gemini', 'GEMINI_API_KEY', 'GOOGLE_API_KEY']],
];

function walk(dir, files = []) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return files;
  }
  for (const entry of entries) {
    if (entry === 'node_modules' || entry === 'target' || entry === '.git') continue;
    const path = join(dir, entry);
    const stat = statSync(path);
    if (stat.isDirectory()) {
      walk(path, files);
    } else if (/\.(rs|ts|tsx|js|json)$/.test(entry)) {
      files.push(path);
    }
  }
  return files;
}

function redact(content) {
  return content
    .replace(/(sk-[A-Za-z0-9_-]{12,})/g, 'sk-REDACTED')
    .replace(/(ya29\.[A-Za-z0-9._-]+)/g, 'ya29.REDACTED')
    .replace(/(Bearer\s+)[A-Za-z0-9._-]{12,}/gi, '$1REDACTED');
}

mkdirSync(outputDir, { recursive: true });

const files = [...new Set(sourceDirs
  .flatMap((sourceDir) => walk(join(upstreamRoot, sourceDir))))]
  .filter((path) => {
    const content = readFileSync(path, 'utf8');
    return interestingNames.some((name) => content.includes(name));
  })
  .sort();

const structures = files.map((path) => {
  const raw = readFileSync(path, 'utf8');
  const content = redact(raw);
  const rel = relative(upstreamRoot, path);
  const coveredFields = Object.fromEntries(
    structuralFields.map(([field, needles]) => [
      field,
      needles.some((needle) => content.includes(needle)),
    ]),
  );
  const lines = content.split(/\r?\n/);
  const sampleLines = lines
    .map((line, index) => ({ index: index + 1, line: line.trim() }))
    .filter(({ line }) => interestingNames.some((name) => line.includes(name)))
    .slice(0, 12);
  return {
    source: rel,
    sha256: createHash('sha256').update(content).digest('hex'),
    coveredFields,
    sampleLines,
  };
});

writeFileSync(
  join(outputDir, 'structures.json'),
  `${JSON.stringify({
    generatedAt: new Date().toISOString(),
    upstreamRoot,
    fields: structuralFields.map(([field]) => field),
    files: structures,
  }, null, 2)}\n`,
);

console.log(`exported ${structures.length} provider structure fixtures to ${join(outputDir, 'structures.json')}`);
