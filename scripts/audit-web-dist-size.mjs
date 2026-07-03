#!/usr/bin/env node
import fs from "node:fs";
import process from "node:process";

const file = "web-dist/index.html";
const maxBytes = Number(process.env.CC_SWITCH_WEB_DIST_MAX_BYTES || 160000);
const size = fs.statSync(file).size;

console.log(`webDistBytes=${size} maxBytes=${maxBytes}`);
if (size > maxBytes) {
  console.error(
    `${file} exceeds the single-file ceiling; split web source or raise the reviewed limit`,
  );
  process.exit(1);
}
