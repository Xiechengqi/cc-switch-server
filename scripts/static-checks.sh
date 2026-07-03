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

echo "== web dist size =="
node scripts/audit-web-dist-size.mjs

echo "== transform coverage audit =="
node scripts/audit-transform-coverage.mjs

echo "== upstream scan =="
node scripts/scan-upstream-changes.mjs --fail-on-must-review

echo "== web inline script parse =="
node - <<'NODE'
const fs = require('fs');
const html = fs.readFileSync('web-dist/index.html', 'utf8');
const scripts = [...html.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/gi)].map((match) => match[1]);
if (!scripts.length) throw new Error('no inline scripts found');
for (const script of scripts) new Function(script);
console.log(`inlineScripts=${scripts.length}`);
NODE

echo "== diff whitespace =="
git diff --check

echo "static checks ok"
