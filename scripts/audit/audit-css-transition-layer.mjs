#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const cssPath = path.join(root, "web-src/src/styles.css");
const srcRoot = path.join(root, "web-src/src");
const maxLines = Number(process.env.CC_SWITCH_STYLES_MAX_LINES || 3000);

const css = fs.readFileSync(cssPath, "utf8");
const sourceText = collectSourceText(srcRoot, cssPath);
const classes = [
  ...new Set(
    [...css.matchAll(/\.(-?[_a-zA-Z]+[_a-zA-Z0-9-]*)/g)]
      .map((match) => match[1])
      .filter((name) => !/^\d/.test(name)),
  ),
].sort();
const unused = classes.filter((name) => !sourceText.includes(name));
const lines = css.split("\n").length - (css.endsWith("\n") ? 1 : 0);

const report = {
  file: "web-src/src/styles.css",
  lines,
  maxLines,
  classes: classes.length,
  unusedClasses: unused.length,
};

console.log(JSON.stringify(report, null, 2));

if (process.argv.includes("--check") && lines > maxLines) {
  console.error(`styles.css transition layer has ${lines} lines; max is ${maxLines}`);
  process.exit(1);
}

function collectSourceText(dir, excludedPath) {
  let output = "";
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      output += collectSourceText(fullPath, excludedPath);
      continue;
    }
    if (!/\.(css|ts|tsx)$/.test(entry.name) || fullPath === excludedPath) {
      continue;
    }
    output += fs.readFileSync(fullPath, "utf8");
    output += "\n";
  }
  return output;
}
