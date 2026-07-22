#!/usr/bin/env node
import fs from "node:fs";
import process from "node:process";

const serverFiles = [
  "src/proxy/transforms.rs",
  "src/proxy/streaming.rs",
  "src/proxy/adapters.rs",
  "src/proxy/stream_transforms.rs",
];

const minimumTests = Number(process.env.CC_SWITCH_TRANSFORM_MIN_TESTS || 216);
const serverTotal = serverFiles
  .map((file) => [file, countRustTests(file)])
  .reduce((sum, [, count]) => sum + count, 0);

const report = {
  minimumTests,
  serverFiles: Object.fromEntries(serverFiles.map((file) => [file, countRustTests(file)])),
  serverTotal,
  remainingToMinimum: Math.max(0, minimumTests - serverTotal),
};

console.log(JSON.stringify(report, null, 2));
if (process.argv.includes("--check") && serverTotal < minimumTests) {
  console.error(
    `transform/streaming fixture coverage ${serverTotal}/${minimumTests} is below the Server baseline`,
  );
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
