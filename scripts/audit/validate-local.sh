#!/usr/bin/env bash
set -euo pipefail

# Match release readiness and CI for large debug async forwarding fixtures.
export RUST_MIN_STACK="${RUST_MIN_STACK:-67108864}"

cargo fmt --check
cargo check
node scripts/audit/audit-claude-wire-profile.mjs
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
node scripts/audit/audit-proxy-bridge-contract.mjs --check
node scripts/audit/audit-web-runtime-contract.mjs
if [[ -d web-src/node_modules ]]; then
  npm --prefix web-src run typecheck
fi
cargo test
