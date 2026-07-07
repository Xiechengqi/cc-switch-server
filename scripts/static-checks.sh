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
  'assets/contract/provider-coverage.json',
  'assets/contract/provider-fixtures/structures.json',
  'assets/contract/web-runtime-contract.json',
  'docs/code-agent-regression-matrix.json',
]) {
  JSON.parse(fs.readFileSync(file, 'utf8'));
}
console.log('json ok');
NODE

echo "== node syntax =="
mapfile -t node_scripts < <(find scripts -type f -name '*.mjs' | sort)
for file in "${node_scripts[@]}"; do
  node --check "$file"
done

echo "== shell syntax =="
mapfile -t shell_scripts < <(find scripts -type f -name '*.sh' | sort)
for file in "${shell_scripts[@]}"; do
  bash -n "$file"
done

echo "== provider audits =="
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
node scripts/audit/audit-web-i18n-literals.mjs
node scripts/audit/audit-web-runtime-contract.mjs

echo "== web dist size =="
node scripts/audit/audit-web-dist-size.mjs

echo "== desktop ui sync drift =="
node scripts/sync/sync-desktop-ui.mjs --check

echo "== web source typecheck =="
if [[ -d web-src && -d web-src/node_modules ]]; then
  npm --prefix web-src run typecheck
else
  echo "skip web source typecheck: web-src/node_modules not installed"
fi

echo "== transform coverage audit =="
node scripts/audit/audit-transform-coverage.mjs

echo "== upstream scan =="
node scripts/sync/scan-upstream-changes.mjs --fail-on-must-review

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

echo "== dependency direction =="
if rg -n 'crate::(http|api)\b' src/proxy; then
  echo 'proxy must not depend on api/http'; exit 1
fi
if rg -n 'crate::clients\b' src/proxy; then
  echo 'proxy must not depend on clients; route outbound client work through state/api orchestration'; exit 1
fi
if rg -n 'crate::(api|http|clients|proxy)\b' src/domain; then
  echo 'domain must stay pure'; exit 1
fi
if rg -n 'crate::(api|http|proxy)\b' src/clients; then
  echo 'clients must not depend on api/proxy'; exit 1
fi
if rg -n 'crate::(api|http|clients|domain|proxy)\b' src/infra; then
  echo 'infra must be the bottom layer'; exit 1
fi

echo "static checks ok"
