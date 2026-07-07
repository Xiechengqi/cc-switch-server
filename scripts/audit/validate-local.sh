#!/usr/bin/env bash
set -euo pipefail

cargo fmt --check
cargo check
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
node scripts/audit/audit-web-runtime-contract.mjs
if [[ -d web-src/node_modules ]]; then
  npm --prefix web-src run typecheck
fi
cargo test
