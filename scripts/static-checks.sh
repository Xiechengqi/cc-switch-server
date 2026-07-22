#!/usr/bin/env bash
set -euo pipefail

echo "== rustfmt check =="
mapfile -t rust_files < <(rg --files -g '*.rs' src)
if [[ "${#rust_files[@]}" -gt 0 ]]; then
  rustfmt --edition 2021 --check "${rust_files[@]}"
fi

echo "== clippy =="
cargo clippy --all-targets -- -D warnings

echo "== json parse =="
node - <<'NODE'
const fs = require('fs');
for (const file of [
  'assets/contract/provider-field-consumption.json',
  'assets/contract/provider-legacy-behavior.json',
  'assets/contract/provider-writer-inventory.json',
  'assets/contract/provider-coverage.json',
  'assets/contract/provider-fixtures/structures.json',
  'assets/contract/router-provider-channel-baseline.json',
  'assets/contract/server-provider-legacy-inventory.json',
  'assets/contract/upstream-provider-source-baseline.json',
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
node --test scripts/audit/*.test.mjs
node scripts/audit/audit-upstream-provider-baseline.mjs --check
node scripts/audit/audit-provider-phase0-contracts.mjs --check
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
node scripts/audit/audit-server-product-boundary.mjs
node scripts/audit/audit-web-i18n-literals.mjs
node scripts/audit/audit-web-runtime-contract.mjs

echo "== web dist size =="
node scripts/audit/audit-web-dist-size.mjs

echo "== web source typecheck =="
if [[ ! -d web-src/node_modules ]]; then
  echo "web source typecheck/tests require web-src/node_modules; run npm --prefix web-src ci"
  exit 1
fi
npm --prefix web-src run typecheck
npm --prefix web-src run test

echo "== transform coverage audit =="
node scripts/audit/audit-transform-coverage.mjs --check

echo "== css transition layer audit =="
node scripts/audit/audit-css-transition-layer.mjs --check

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
git diff --check -- ':(exclude)web-dist'

echo "== state write discipline =="
state_write_paths=(src/api src/clients src/domain src/proxy src/infra tests)
if rg -n -U 'state\s*\.\s*(config|providers|accounts|usage|shares|ui_settings|sessions|oauth_logins)\s*\.\s*write\s*\(\s*\)\s*\.\s*await' "${state_write_paths[@]}"; then
  echo 'state store writes must go through ServerStateInner domain methods'; exit 1
fi
if rg -n -U 'state\s*\.\s*save_(providers|accounts|usage|ui_settings)\s*\(\s*\)\s*\.\s*await|save_(accounts|shares)_debounced\s*\(' "${state_write_paths[@]}"; then
  echo 'state store persistence must stay encapsulated in ServerStateInner domain methods'; exit 1
fi

echo "== direct outbound HTTP policy =="
if rg -n 'reqwest::Proxy|\.proxy\s*\(' src; then
  echo 'server outbound HTTP must remain direct; explicit proxy construction is forbidden'; exit 1
fi
if ! rg -U -q 'reqwest::Client::builder\(\)\s*\.no_proxy\(\)' src/infra/http.rs; then
  echo 'central HTTP client builder must explicitly disable environment/system proxies'; exit 1
fi

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
