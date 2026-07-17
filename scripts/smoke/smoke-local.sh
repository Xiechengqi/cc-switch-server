#!/usr/bin/env bash
set -euo pipefail

PORT="${PORT:-18082}"
HOST="${HOST:-127.0.0.1}"
CONFIG_DIR="${CONFIG_DIR:-$(mktemp -d /tmp/cc-switch-server-smoke.XXXXXX)}"
SERVER_URL="http://${HOST}:${PORT}"
LOG_FILE="${CONFIG_DIR}/server.log"

cleanup() {
  if [[ -n "${PID:-}" ]]; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  if [[ "${KEEP_CONFIG_DIR:-0}" != "1" ]]; then
    rm -rf "$CONFIG_DIR"
  else
    echo "kept config dir: $CONFIG_DIR"
  fi
}
trap cleanup EXIT

cargo run -- --host "$HOST" --port "$PORT" --config-dir "$CONFIG_DIR" >"$LOG_FILE" 2>&1 &
PID=$!

for _ in $(seq 1 30); do
  if curl -fsS "$SERVER_URL/health" >/tmp/cc-switch-server-smoke-health.json 2>/dev/null; then
    break
  fi
  sleep 1
done

echo "== health =="
curl -fsS "$SERVER_URL/health"
echo

echo "== version =="
curl -fsS "$SERVER_URL/version"
echo

echo "== web fallback =="
WEB_BODY="${CONFIG_DIR}/web-fallback.out"
WEB_STATUS="$(curl -sS -o "$WEB_BODY" -w "%{http_code}" "$SERVER_URL/" || true)"
echo "status: $WEB_STATUS"
head -c 160 "$WEB_BODY"
echo
if [[ "$WEB_STATUS" != "200" && "$WEB_STATUS" != "404" ]]; then
  echo "unexpected web fallback status: $WEB_STATUS" >&2
  exit 1
fi

echo "== setup =="
curl -fsS -X POST \
  -H "Content-Type: application/json" \
  -d '{"password":"password123","ownerEmail":"owner@example.com","routerUrl":"http://127.0.0.1:9","clientTunnelSubdomain":"ownertest","options":{"allowOffline":true}}' \
  "$SERVER_URL/api/setup"
echo

echo "== password login =="
TOKEN="$(curl -fsS -X POST \
  -H "Content-Type: application/json" \
  -d '{"method":"password","password":"password123"}' \
  "$SERVER_URL/api/auth/login" | node -e 'let s="";process.stdin.on("data",d=>s+=d);process.stdin.on("end",()=>process.stdout.write(JSON.parse(s).token))')"
echo "token length: ${#TOKEN}"

echo "== auth me =="
curl -fsS -H "Authorization: Bearer $TOKEN" "$SERVER_URL/api/auth/me"
echo

echo "== rotate api token =="
API_TOKEN="$(curl -fsS -X POST \
  -H "Authorization: Bearer $TOKEN" \
  "$SERVER_URL/api/auth/api-token" | node -e 'let s="";process.stdin.on("data",d=>s+=d);process.stdin.on("end",()=>process.stdout.write(JSON.parse(s).apiToken))')"
echo "api token length: ${#API_TOKEN}"

echo "== api token login =="
curl -fsS -X POST \
  -H "Content-Type: application/json" \
  -d "{\"method\":\"api_token\",\"apiToken\":\"$API_TOKEN\"}" \
  "$SERVER_URL/api/auth/login"
echo

echo "== create share =="
curl -fsS -X POST \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"id":"smoke-share","app":"codex","providerId":"smoke-provider","providerType":"codex","displayName":"Smoke Share"}' \
  "$SERVER_URL/api/shares"
echo

echo "== update share market grant =="
curl -fsS -X POST \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"marketGrant":{"status":"pending","grantId":"smoke-grant"}}' \
  "$SERVER_URL/api/shares/smoke-share/market-grant"
echo
