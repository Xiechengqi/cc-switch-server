#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEB_SRC_DIR="$ROOT_DIR/web-src"
WEB_DIST_DIR="${WEB_DIST_DIR:-$ROOT_DIR/web-dist}"

if [[ ! -d "$WEB_SRC_DIR" ]]; then
  echo "web source directory not found: $WEB_SRC_DIR" >&2
  exit 1
fi

if [[ "${SKIP_NPM_INSTALL:-0}" != "1" ]]; then
  if [[ -f "$WEB_SRC_DIR/package-lock.json" ]]; then
    npm --prefix "$WEB_SRC_DIR" ci --ignore-scripts --no-audit --no-fund
  else
    npm --prefix "$WEB_SRC_DIR" install --ignore-scripts --no-audit --no-fund
  fi
fi

npm --prefix "$WEB_SRC_DIR" run typecheck
WEB_DIST_DIR="$WEB_DIST_DIR" npm --prefix "$WEB_SRC_DIR" run build
