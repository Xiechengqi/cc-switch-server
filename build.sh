#!/usr/bin/env bash
set -euo pipefail -x

cd "$(dirname "$0")"

npm --prefix web-src ci --ignore-scripts --no-audit --no-fund
WEB_DIST_DIR="$PWD/web-dist" npm --prefix web-src run build

rm -f -v target/release/cc-switch-server

cargo build --release --locked

cp -f -v target/release/cc-switch-server .
