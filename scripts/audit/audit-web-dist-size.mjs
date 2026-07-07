#!/usr/bin/env node
import fs from "node:fs";
import process from "node:process";

const root = "web-dist";
const indexFile = `${root}/index.html`;
// Phase U12: full desktop UI parity (4 locales + CodeMirror + recharts) ships ~4.2MB raw.
const maxBytes = Number(process.env.CC_SWITCH_WEB_DIST_MAX_BYTES || 4503592);

function fileSize(path) {
  return fs.statSync(path).size;
}

function walk(dir) {
  const files = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = `${dir}/${entry.name}`;
    if (entry.isDirectory()) {
      files.push(...walk(fullPath));
    } else if (entry.isFile()) {
      files.push(fullPath);
    }
  }
  return files;
}

if (!fs.existsSync(indexFile)) {
  console.error(`${indexFile} is required`);
  process.exit(1);
}

const files = walk(root);
const size = files.reduce((sum, file) => sum + fileSize(file), 0);

console.log(`webDistFiles=${files.length} webDistBytes=${size} maxBytes=${maxBytes}`);
if (size > maxBytes) {
  console.error(
    `${root} exceeds the reviewed embedded web asset ceiling`,
  );
  process.exit(1);
}
