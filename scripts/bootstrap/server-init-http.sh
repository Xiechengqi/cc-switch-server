#!/usr/bin/env bash
set -euo pipefail

SERVER_URL="${CC_SWITCH_SERVER_URL:-http://127.0.0.1:15721}"
OWNER_EMAIL="${CC_SWITCH_OWNER_EMAIL:?CC_SWITCH_OWNER_EMAIL is required}"
ADMIN_PASSWORD="${CC_SWITCH_ADMIN_PASSWORD:?CC_SWITCH_ADMIN_PASSWORD is required}"
ROUTER_URL="${CC_SWITCH_ROUTER_URL:?CC_SWITCH_ROUTER_URL is required}"
CLIENT_SUBDOMAIN="${CC_SWITCH_CLIENT_SUBDOMAIN:-}"

echo "== wait for health =="
for _ in $(seq 1 40); do
  if curl -fsS "${SERVER_URL}/health" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl -fsS "${SERVER_URL}/health" >/dev/null

echo "== setup status =="
STATUS_JSON="$(curl -fsS "${SERVER_URL}/api/setup/status")"
echo "${STATUS_JSON}"
if ! node -e 'const s=JSON.parse(process.argv[1]); process.exit(s.needsSetup?0:2)' "${STATUS_JSON}"; then
  echo "server setup already complete"
  exit 0
fi

PAYLOAD="$(node -e '
const payload = {
  password: process.argv[1],
  ownerEmail: process.argv[2],
  routerUrl: process.argv[3],
  clientTunnelSubdomain: process.argv[4] || "",
};
process.stdout.write(JSON.stringify(payload));
' "${ADMIN_PASSWORD}" "${OWNER_EMAIL}" "${ROUTER_URL}" "${CLIENT_SUBDOMAIN}")"

echo "== bootstrap setup =="
RESPONSE="$(curl -fsS -X POST "${SERVER_URL}/api/setup/bootstrap" \
  -H 'content-type: application/json' \
  -d "${PAYLOAD}")"
echo "${RESPONSE}"

export CC_SWITCH_SERVER_TOKEN="$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).sessionToken||"")' "${RESPONSE}")"
if [[ -z "${CC_SWITCH_SERVER_TOKEN}" ]]; then
  echo "bootstrap response did not include sessionToken" >&2
  exit 1
fi

echo "session token length: ${#CC_SWITCH_SERVER_TOKEN}"
