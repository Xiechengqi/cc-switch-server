#!/usr/bin/env bash
set -euo pipefail

CONFIG_DIR="${CC_SWITCH_SERVER_CONFIG_DIR:-${HOME}/.cc-switch-server}"
OWNER_EMAIL="${CC_SWITCH_OWNER_EMAIL:?CC_SWITCH_OWNER_EMAIL is required}"
ADMIN_PASSWORD="${CC_SWITCH_ADMIN_PASSWORD:?CC_SWITCH_ADMIN_PASSWORD is required}"
ROUTER_URL="${CC_SWITCH_ROUTER_URL:?CC_SWITCH_ROUTER_URL is required}"
CLIENT_SUBDOMAIN="${CC_SWITCH_CLIENT_SUBDOMAIN:-}"
BIN="${CC_SWITCH_SERVER_BIN:-cc-switch-server}"

INIT_ARGS=(
  --config-dir "${CONFIG_DIR}"
  init
  --owner-email "${OWNER_EMAIL}"
  --router-url "${ROUTER_URL}"
  --password "${ADMIN_PASSWORD}"
)

if [[ -n "${CLIENT_SUBDOMAIN}" ]]; then
  INIT_ARGS+=(--client-subdomain "${CLIENT_SUBDOMAIN}")
fi

echo "== cli init =="
"${BIN}" "${INIT_ARGS[@]}"

echo "== config validate =="
"${BIN}" --config-dir "${CONFIG_DIR}" config validate

echo "setup written to ${CONFIG_DIR}"
