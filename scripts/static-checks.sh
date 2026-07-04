#!/usr/bin/env bash
set -euo pipefail

echo "== rustfmt check =="
mapfile -t rust_files < <(rg --files -g '*.rs' src)
if [[ "${#rust_files[@]}" -gt 0 ]]; then
  rustfmt --edition 2021 --check "${rust_files[@]}"
fi

echo "== json parse =="
node - <<'NODE'
const fs = require('fs');
for (const file of [
  'docs/code-agent-regression-matrix.json',
  'docs/provider-coverage.json',
  'docs/provider-fixtures/structures.json',
]) {
  JSON.parse(fs.readFileSync(file, 'utf8'));
}
console.log('json ok');
NODE

echo "== node syntax =="
node --check scripts/audit-provider-coverage.mjs
node --check scripts/audit-transform-coverage.mjs
node --check scripts/audit-ui-provider-matrix.mjs
node --check scripts/audit-web-i18n-literals.mjs
node --check scripts/audit-web-runtime-contract.mjs
node --check scripts/audit-web-dist-size.mjs
node --check scripts/import-desktop-export.mjs
node --check scripts/scan-upstream-changes.mjs
node --check scripts/write-acceptance-evidence.mjs

echo "== shell syntax =="
for file in scripts/*.sh; do
  bash -n "$file"
done

echo "== provider audits =="
node scripts/audit-provider-coverage.mjs --check
node scripts/audit-ui-provider-matrix.mjs --check
node scripts/audit-web-i18n-literals.mjs
node scripts/audit-web-runtime-contract.mjs

echo "== web dist size =="
node scripts/audit-web-dist-size.mjs

echo "== web source typecheck =="
if [[ -d web-src && -d web-src/node_modules ]]; then
  npm --prefix web-src run typecheck
else
  echo "skip web source typecheck: web-src/node_modules not installed"
fi

echo "== transform coverage audit =="
node scripts/audit-transform-coverage.mjs

echo "== upstream scan =="
node scripts/scan-upstream-changes.mjs --fail-on-must-review

echo "== web dist asset references =="
node - <<'NODE'
const fs = require('fs');
const path = require('path');
const html = fs.readFileSync('web-dist/index.html', 'utf8');
const scripts = [...html.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/gi)].map((match) => match[1]);
for (const script of scripts) new Function(script);

const refs = [];
for (const match of html.matchAll(/<(?:script|link)\b[^>]*(?:src|href)=["']([^"']+)["'][^>]*>/gi)) {
  const ref = match[1];
  if (/^(?:https?:)?\/\//.test(ref) || ref.startsWith('/')) continue;
  refs.push(ref.replace(/^\.\//, ''));
}
for (const ref of refs) {
  const fullPath = path.join('web-dist', ref);
  if (!fs.existsSync(fullPath)) throw new Error(`missing web-dist asset ${ref}`);
}
if (!scripts.length && !refs.length) throw new Error('no inline or external web assets found');
console.log(`inlineScripts=${scripts.length} assetRefs=${refs.length}`);
NODE

echo "== diff whitespace =="
git diff --check

echo "static checks ok"
