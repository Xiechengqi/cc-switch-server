#!/usr/bin/env node
import fs from "node:fs";
import process from "node:process";

const desktopBaseline = {
  transform_codex_chat: 55,
  transform_responses: 61,
  transform: 59,
  transform_gemini: 26,
  streaming: 52,
};

const serverFiles = [
  "src/proxy/transforms.rs",
  "src/proxy/streaming.rs",
  "src/proxy/adapters.rs",
];

const targetRatio = Number(process.env.CC_SWITCH_TRANSFORM_COVERAGE_TARGET || 0.7);
const desktopTotal = Object.values(desktopBaseline).reduce((sum, value) => sum + value, 0);
const target = Math.ceil(desktopTotal * targetRatio);
const serverTotal = serverFiles
  .map((file) => [file, countRustTests(file)])
  .reduce((sum, [, count]) => sum + count, 0);

const report = {
  desktopBaseline,
  desktopTotal,
  targetRatio,
  target,
  serverFiles: Object.fromEntries(serverFiles.map((file) => [file, countRustTests(file)])),
  serverTotal,
  remainingToTarget: Math.max(0, target - serverTotal),
};

console.log(JSON.stringify(report, null, 2));
if (process.argv.includes("--check") && serverTotal < target) {
  console.error(`transform/streaming fixture coverage ${serverTotal}/${target} is below target`);
  process.exit(1);
}

function countRustTests(file) {
  const text = fs.readFileSync(file, "utf8");
  const directTests = [...text.matchAll(/#\s*\[\s*test\s*\]/g)].length;
  const generatedUsageCases = [
    ...text.matchAll(/\b(?:openai|claude|codex|gemini)_usage_case!\s*\(/g),
  ].length;
  return directTests + generatedUsageCases;
}
