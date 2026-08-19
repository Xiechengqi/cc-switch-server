#!/usr/bin/env bash
set -euo pipefail

# Client + Router smoke gate.  This script intentionally has no dependency on
# the retired standalone Token Market, its URL, credentials, or bearer API.
ROUTER_BASE_URL="${ROUTER_BASE_URL:-}"
ROUTER_API_TOKEN="${ROUTER_API_TOKEN:-}"
ROUTER_API_TOKEN_HEADER="${ROUTER_API_TOKEN_HEADER:-Authorization}"
CC_SWITCH_SHARE_URL="${CC_SWITCH_SHARE_URL:-}"
SHARE_ID="${SHARE_ID:-}"
RUN_REAL="${RUN_REAL:-0}"

pass() { echo "[PASS] $*"; }
warn() { echo "[WARN] $*"; }
fail() { echo "[FAIL] $*" >&2; return 1; }

if [[ -z "$ROUTER_BASE_URL" || "$ROUTER_BASE_URL" == \<* ]]; then
  warn "ROUTER_BASE_URL missing; Router Share smoke blocked-inputs"
  exit 0
fi

router_base="${ROUTER_BASE_URL%/}"
auth_args=()
if [[ -n "$ROUTER_API_TOKEN" && "$ROUTER_API_TOKEN" != \<* ]]; then
  case "$ROUTER_API_TOKEN_HEADER" in
    Authorization|authorization) auth_args=(-H "Authorization: Bearer $ROUTER_API_TOKEN") ;;
    x-api-key|X-API-Key|x-goog-api-key|X-Goog-Api-Key) auth_args=(-H "$ROUTER_API_TOKEN_HEADER: $ROUTER_API_TOKEN") ;;
    *) fail "unsupported ROUTER_API_TOKEN_HEADER: $ROUTER_API_TOKEN_HEADER" ;;
  esac
fi

probe_get() {
  local label="$1" url="$2" expected="$3" out status
  out="$(mktemp /tmp/cc-switch-router-share-smoke.XXXXXX)"
  status="$(curl -sS -L --max-time 30 -o "$out" -w '%{http_code}' "${auth_args[@]}" "$url" || true)"
  echo "$label: status=$status"
  sed -n '1,8p' "$out"
  rm -f "$out"
  [[ "$status" =~ $expected ]] || fail "$label returned $status"
  pass "$label"
}

probe_get "router health" "$router_base/v1/healthz" '^2'
probe_get "retired Token Market route" "$router_base/v1/markets" '^410$'

if [[ -n "$CC_SWITCH_SHARE_URL" && "$CC_SWITCH_SHARE_URL" != \<* ]]; then
  share_base="${CC_SWITCH_SHARE_URL%/}"
  probe_get "share route" "$share_base/health" '^2'
else
  warn "CC_SWITCH_SHARE_URL missing; skipped Share route probe"
fi

if [[ -n "$SHARE_ID" && "$SHARE_ID" != \<* ]]; then
  echo "shareId=$SHARE_ID"
else
  warn "SHARE_ID missing; Share identity fixture is not available"
fi

if [[ "$RUN_REAL" != "1" ]]; then
  echo "[INFO] RUN_REAL=0; no provider or billing success is claimed."
fi
pass "Client + Router Share smoke completed"
