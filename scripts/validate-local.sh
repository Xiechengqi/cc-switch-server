#!/usr/bin/env bash
set -euo pipefail

cargo fmt --check
cargo check
node scripts/audit-provider-coverage.mjs --check
node scripts/audit-ui-provider-matrix.mjs --check
cargo test
