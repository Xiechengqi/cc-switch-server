#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const stylesPath = path.join(root, "web-src/src/styles.css");
const stylesDir = path.join(root, "web-src/src/styles");
const srcRoot = path.join(root, "web-src/src");
const maxCoreLines = Number(process.env.CC_SWITCH_STYLES_MAX_LINES || 1200);
const maxUnusedClasses = Number(process.env.CC_SWITCH_STYLES_MAX_UNUSED || 35);

function collectSourceText(dir, excludedPath) {
  let output = "";
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "i18n" || entry.name === "styles") {
        continue;
      }
      output += collectSourceText(fullPath, excludedPath);
      continue;
    }
    if (/\.(tsx|ts)$/.test(entry.name) && fullPath !== excludedPath) {
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

function readTransitionCssFiles() {
  const files = ["web-src/src/styles.css"];
  if (fs.existsSync(stylesDir)) {
    for (const entry of fs.readdirSync(stylesDir)) {
      if (entry.endsWith(".css")) {
        files.push(path.join("web-src/src/styles", entry));
      }
    }
  }
  return files;
}

const transitionFiles = readTransitionCssFiles();
const css = transitionFiles
  .map((relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8"))
  .join("\n");
const coreCss = fs.readFileSync(stylesPath, "utf8");
const sourceText = collectSourceText(srcRoot, stylesPath);
const classes = [
  ...new Set(
    [...css.matchAll(/\.(-?[_a-zA-Z]+[_a-zA-Z0-9-]*)/g)]
      .map((match) => match[1])
      .filter((name) => !/^\d/.test(name)),
  ),
].sort();
const unused = classes.filter((name) => !classIsReferenced(name, sourceText));
const lines = coreCss.split("\n").length - (coreCss.endsWith("\n") ? 1 : 0);
const moduleLines = Object.fromEntries(
  transitionFiles
    .filter((file) => file !== "web-src/src/styles.css")
    .map((file) => {
      const content = fs.readFileSync(path.join(root, file), "utf8");
      return [
        file,
        content.split("\n").length - (content.endsWith("\n") ? 1 : 0),
      ];
    }),
);

const report = {
  file: "web-src/src/styles.css",
  lines,
  maxLines: maxCoreLines,
  moduleFiles: moduleLines,
  transitionTotalLines:
    lines + Object.values(moduleLines).reduce((sum, count) => sum + count, 0),
  classes: classes.length,
  unusedClasses: unused.length,
};

console.log(JSON.stringify(report, null, 2));

if (process.argv.includes("--check") && lines > maxCoreLines) {
  console.error(`styles.css transition core has ${lines} lines; max is ${maxCoreLines}`);
  process.exit(1);
}

if (process.argv.includes("--check") && unused.length > maxUnusedClasses) {
  console.error(
    `styles.css has ${unused.length} unused transition classes; max is ${maxUnusedClasses}`,
  );
  process.exit(1);
}
