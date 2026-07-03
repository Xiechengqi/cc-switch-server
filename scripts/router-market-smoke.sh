#!/usr/bin/env bash
set -euo pipefail

# Required:
#   CC_SWITCH_SERVER_TOKEN  Bearer token from /api/auth/login.
#
# Optional:
#   SERVER_URL              cc-switch-server base URL. Default: http://127.0.0.1:15721
#   SHARE_ID                Local server share id for direct binding probe.
#   DIRECT_SHARE_URL        Public direct share tunnel URL, without trailing /v1/responses.
#   MARKET_API_URL          Market API URL, without trailing /v1/responses.
#   MARKET_URL              Market admin/base URL for health probe.
#   ROUTER_API_TOKEN        Router user API token for public direct/market URL probes.
#   ROUTER_API_TOKEN_HEADER Header name for ROUTER_API_TOKEN. Default: Authorization.
#                           Router accepts Authorization: Bearer, x-api-key, x-goog-api-key.
#   MARKET_API_TOKEN        Optional market-specific token. Defaults to ROUTER_API_TOKEN when unset.
#   MARKET_API_TOKEN_HEADER Header name for MARKET_API_TOKEN. Defaults to ROUTER_API_TOKEN_HEADER.
#   PROBE_MODEL             Model used by probe payloads. Default: probe.
#   STREAM_PROBE            Set to 1 to run optional stream probes.
#   REQUIRE_STREAM_USAGE    Set to 1 to require usage in stream summaries. Default: 0.
#   EVIDENCE_FILE           Optional redacted JSON evidence output path.
#
SERVER_URL="${SERVER_URL:-http://127.0.0.1:15721}"
MARKET_URL="${MARKET_URL:-}"
API_TOKEN="${CC_SWITCH_SERVER_TOKEN:-}"
SHARE_ID="${SHARE_ID:-}"
DIRECT_SHARE_URL="${DIRECT_SHARE_URL:-}"
MARKET_API_URL="${MARKET_API_URL:-}"
ROUTER_API_TOKEN="${ROUTER_API_TOKEN:-}"
ROUTER_API_TOKEN_HEADER="${ROUTER_API_TOKEN_HEADER:-Authorization}"
MARKET_API_TOKEN="${MARKET_API_TOKEN:-}"
MARKET_API_TOKEN_HEADER="${MARKET_API_TOKEN_HEADER:-}"
PROBE_MODEL="${PROBE_MODEL:-probe}"
STREAM_PROBE="${STREAM_PROBE:-0}"
REQUIRE_STREAM_USAGE="${REQUIRE_STREAM_USAGE:-0}"
EVIDENCE_FILE="${EVIDENCE_FILE:-}"
FAILURES=0
WARNINGS=0
LOCAL_SHARE_STATUS=""
DIRECT_NOAUTH_STATUS=""
DIRECT_PUBLIC_STATUS=""
DIRECT_PUBLIC_STREAM_STATUS=""
MARKET_API_STATUS=""
MARKET_API_STREAM_STATUS=""
MARKET_HEALTH_STATUS=""

if [[ -z "$API_TOKEN" ]]; then
  echo "CC_SWITCH_SERVER_TOKEN is required" >&2
  exit 2
fi

auth_header=(-H "Authorization: Bearer $API_TOKEN")
router_auth_header=()
if [[ -n "$ROUTER_API_TOKEN" ]]; then
  case "$ROUTER_API_TOKEN_HEADER" in
    Authorization|authorization)
      router_auth_header=(-H "Authorization: Bearer $ROUTER_API_TOKEN")
      ;;
    x-api-key|X-API-Key|x-goog-api-key|X-Goog-Api-Key)
      router_auth_header=(-H "$ROUTER_API_TOKEN_HEADER: $ROUTER_API_TOKEN")
      ;;
    *)
      echo "unsupported ROUTER_API_TOKEN_HEADER: $ROUTER_API_TOKEN_HEADER" >&2
      exit 2
      ;;
  esac
fi

market_auth_header=("${router_auth_header[@]}")
if [[ -n "$MARKET_API_TOKEN" ]]; then
  market_header="${MARKET_API_TOKEN_HEADER:-Authorization}"
  case "$market_header" in
    Authorization|authorization)
      market_auth_header=(-H "Authorization: Bearer $MARKET_API_TOKEN")
      ;;
    x-api-key|X-API-Key|x-goog-api-key|X-Goog-Api-Key)
      market_auth_header=(-H "$market_header: $MARKET_API_TOKEN")
      ;;
    *)
      echo "unsupported MARKET_API_TOKEN_HEADER: $market_header" >&2
      exit 2
      ;;
  esac
fi

pass() {
  echo "[PASS] $*"
}

warn() {
  WARNINGS=$((WARNINGS + 1))
  echo "[WARN] $*"
}

fail() {
  FAILURES=$((FAILURES + 1))
  echo "[FAIL] $*"
}

probe_payload() {
  node -e '
const input = process.argv[1];
const stream = process.argv[2] === "1";
process.stdout.write(JSON.stringify({
  model: process.env.PROBE_MODEL || "probe",
  input,
  stream
}));
' "$1" "$2"
}

fetch_required() {
  local label="$1"
  local url="$2"
  shift 2
  local body_file
  local status
  body_file="$(mktemp /tmp/cc-switch-server-smoke-fetch.XXXXXX)"
  status="$(curl -sS -o "$body_file" -w "%{http_code}" "$@" "$url" || true)"
  cat "$body_file"
  echo
  if [[ "$status" =~ ^2 ]] && ! json_ok_false "$body_file"; then
    pass "$label"
  else
    fail "$label"
  fi
  rm -f "$body_file"
}

fetch_optional() {
  local label="$1"
  local url="$2"
  shift 2
  local body_file
  local status
  body_file="$(mktemp /tmp/cc-switch-server-smoke-fetch.XXXXXX)"
  status="$(curl -sS -o "$body_file" -w "%{http_code}" "$@" "$url" || true)"
  cat "$body_file"
  echo
  if [[ "$status" =~ ^2 ]] && ! json_ok_false "$body_file"; then
    pass "$label"
  else
    warn "$label"
  fi
  rm -f "$body_file"
}

post_optional() {
  local label="$1"
  local url="$2"
  shift 2
  local body_file
  local status
  body_file="$(mktemp /tmp/cc-switch-server-smoke-post.XXXXXX)"
  status="$(curl -sS -o "$body_file" -w "%{http_code}" -X POST "$@" "$url" || true)"
  cat "$body_file"
  echo
  if [[ "$status" =~ ^2 ]] && ! json_ok_false "$body_file"; then
    pass "$label"
  else
    warn "$label"
  fi
  rm -f "$body_file"
}

stream_optional() {
  local label="$1"
  local url="$2"
  local body="$3"
  shift 3
  local args summary_file status ok
  args=(--url "$url" --body "$body" --require-done)
  if [[ "$REQUIRE_STREAM_USAGE" == "1" ]]; then
    args+=(--require-usage)
  fi
  while [[ "$#" -gt 0 ]]; do
    case "$1" in
      -H)
        args+=(--header "$2")
        shift 2
        ;;
      *)
        shift
        ;;
    esac
  done

  summary_file="$(mktemp /tmp/cc-switch-server-stream-summary.XXXXXX)"
  if node scripts/stream-probe.mjs "${args[@]}" >"$summary_file"; then
    ok=1
  else
    ok=0
  fi
  cat "$summary_file"
  status="$(node -e '
const fs = require("fs");
try {
  const data = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  process.stdout.write(String(data.status || ""));
} catch {
  process.stdout.write("");
}
' "$summary_file")"
  rm -f "$summary_file"
  echo
  if [[ "$ok" == "1" ]]; then
    pass "$label"
  else
    warn "$label"
  fi
  STREAM_OPTIONAL_STATUS="$status"
}

json_ok_false() {
  local file="$1"
  node -e '
const fs = require("fs");
try {
  const data = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  process.exit(data && data.ok === false ? 0 : 1);
} catch {
  process.exit(1);
}
' "$file"
}

echo "== server health =="
fetch_required "server health" "$SERVER_URL/health"

echo "== router status =="
fetch_required "router status" "$SERVER_URL/api/router/status" "${auth_header[@]}"

echo "== router diagnostics =="
fetch_required "router diagnostics" "$SERVER_URL/api/router/diagnostics" "${auth_header[@]}"

echo "== router tunnels =="
fetch_required "router tunnels" "$SERVER_URL/api/router/tunnels" "${auth_header[@]}"

echo "== router register =="
post_optional "router register" "$SERVER_URL/api/router/register" "${auth_header[@]}"

echo "== client tunnel start =="
post_optional "client tunnel start" "$SERVER_URL/api/router/client-tunnel/lease" "${auth_header[@]}"

echo "== router batch sync =="
post_optional "router batch sync" "$SERVER_URL/api/router/batch-sync" "${auth_header[@]}"

echo "== router pending share edits pull =="
post_optional "router pending share edits pull" "$SERVER_URL/api/router/share-edits/pull" "${auth_header[@]}"

echo "== shares before probes =="
fetch_required "shares before probes" "$SERVER_URL/api/shares" "${auth_header[@]}"

echo "== share runtime snapshot refresh =="
post_optional "share runtime snapshot refresh" "$SERVER_URL/api/shares/runtime-snapshot" "${auth_header[@]}"

if [[ -n "$SHARE_ID" ]]; then
  echo "== direct share codex probe =="
  direct_local_payload="$(probe_payload "ping" 0)"
  direct_status="$(curl -sS -o /tmp/cc-switch-server-direct-share.out -w "%{http_code}" \
    -H "Content-Type: application/json" \
    -H "X-CC-Switch-Share-Id: $SHARE_ID" \
    -H "X-CC-Switch-Data-Source: direct" \
    -d "$direct_local_payload" \
    "$SERVER_URL/v1/responses" || true)"
  LOCAL_SHARE_STATUS="$direct_status"
  echo "status=${direct_status}"
  cat /tmp/cc-switch-server-direct-share.out
  echo
  if [[ "$direct_status" =~ ^[0-9]{3}$ ]]; then
    pass "direct share local probe reached server"
  else
    warn "direct share local probe did not return an HTTP status"
  fi
else
  warn "SHARE_ID not set; skipped local direct share binding probe"
fi

if [[ -n "$DIRECT_SHARE_URL" ]]; then
  echo "== direct share internal router health probe =="
  fetch_optional "direct share internal router health probe" "$DIRECT_SHARE_URL/_share-router/health" \
    -H "X-Share-Router-Probe: 1"

  echo "== direct share internal request logs probe =="
  fetch_optional "direct share internal request logs probe" "$DIRECT_SHARE_URL/_share-router/request-logs?limit=5"

  if [[ -n "$SHARE_ID" ]]; then
    echo "== direct share internal runtime probe =="
    fetch_optional "direct share internal runtime probe" "$DIRECT_SHARE_URL/_share-router/share-runtime?shareId=$SHARE_ID" \
      -H "X-Share-Router-Probe: 1"
  else
    warn "SHARE_ID not set; skipped direct share internal runtime probe"
  fi

  echo "== direct share public url missing token probe =="
  direct_noauth_payload="$(probe_payload "ping" 0)"
  direct_noauth_status="$(curl -LsS -o /tmp/cc-switch-server-direct-public-noauth.out -w "%{http_code}" \
    -H "Content-Type: application/json" \
    -d "$direct_noauth_payload" \
    "$DIRECT_SHARE_URL/v1/responses" || true)"
  DIRECT_NOAUTH_STATUS="$direct_noauth_status"
  echo "status=${direct_noauth_status}"
  cat /tmp/cc-switch-server-direct-public-noauth.out
  echo
  if [[ "$direct_noauth_status" == "401" ]] && grep -qi "missing-router-api-token" /tmp/cc-switch-server-direct-public-noauth.out; then
    pass "direct share public url rejects missing router api token"
  else
    warn "direct share public url missing-token response was not 401 missing-router-api-token"
  fi

  echo "== direct share public url codex probe =="
  if [[ -z "$ROUTER_API_TOKEN" ]]; then
    warn "ROUTER_API_TOKEN not set; skipped authenticated public direct share probe"
  else
    direct_public_payload="$(probe_payload "ping" 0)"
    direct_public_status="$(curl -LsS -o /tmp/cc-switch-server-direct-public.out -w "%{http_code}" \
      -H "Content-Type: application/json" \
      "${router_auth_header[@]}" \
      -d "$direct_public_payload" \
      "$DIRECT_SHARE_URL/v1/responses" || true)"
    DIRECT_PUBLIC_STATUS="$direct_public_status"
    echo "status=${direct_public_status}"
    cat /tmp/cc-switch-server-direct-public.out
    echo
    if [[ "$direct_public_status" =~ ^2 ]]; then
      pass "direct share public url succeeded"
    else
      warn "direct share public url did not succeed"
    fi
    if [[ "$STREAM_PROBE" == "1" ]]; then
      echo "== direct share public url codex stream probe =="
      direct_stream_payload="$(probe_payload "stream ping" 1)"
      STREAM_OPTIONAL_STATUS=""
      stream_optional "direct share public stream url succeeded" "$DIRECT_SHARE_URL/v1/responses" \
        "$direct_stream_payload" "${router_auth_header[@]}"
      DIRECT_PUBLIC_STREAM_STATUS="$STREAM_OPTIONAL_STATUS"
    fi
  fi
else
  warn "DIRECT_SHARE_URL not set; skipped public direct share probe"
fi

if [[ -n "$MARKET_API_URL" ]]; then
  echo "== market api url codex probe =="
  if [[ -z "$ROUTER_API_TOKEN" && -z "$MARKET_API_TOKEN" ]]; then
    warn "ROUTER_API_TOKEN/MARKET_API_TOKEN not set; skipped authenticated market api probe"
  else
    market_api_payload="$(probe_payload "ping" 0)"
    market_api_status="$(curl -LsS -o /tmp/cc-switch-server-market-api.out -w "%{http_code}" \
      -H "Content-Type: application/json" \
      "${market_auth_header[@]}" \
      -d "$market_api_payload" \
      "$MARKET_API_URL/v1/responses" || true)"
    MARKET_API_STATUS="$market_api_status"
    echo "status=${market_api_status}"
    cat /tmp/cc-switch-server-market-api.out
    echo
    if [[ "$market_api_status" =~ ^2 ]]; then
      pass "market api url succeeded"
    else
      warn "market api url did not succeed"
    fi
    if [[ "$STREAM_PROBE" == "1" ]]; then
      echo "== market api url codex stream probe =="
      market_stream_payload="$(probe_payload "stream ping" 1)"
      STREAM_OPTIONAL_STATUS=""
      stream_optional "market api stream url succeeded" "$MARKET_API_URL/v1/responses" \
        "$market_stream_payload" "${market_auth_header[@]}"
      MARKET_API_STREAM_STATUS="$STREAM_OPTIONAL_STATUS"
    fi
  fi
else
  warn "MARKET_API_URL not set; skipped market api probe"
fi

if [[ -n "$MARKET_URL" ]]; then
  echo "== market health =="
  MARKET_HEALTH_STATUS="$(curl -sS -o /tmp/cc-switch-server-market-health.out -w "%{http_code}" "$MARKET_URL/health" || true)"
  cat /tmp/cc-switch-server-market-health.out
  if [[ "$MARKET_HEALTH_STATUS" =~ ^2 ]]; then
    echo
    pass "market health"
  else
    echo
    warn "market health"
  fi
else
  warn "MARKET_URL not set; skipped market health"
fi

echo "== pending router request log retry =="
post_optional "pending router request log retry" "$SERVER_URL/api/usage/router-sync/retry" "${auth_header[@]}"

echo "== shares after sync =="
fetch_required "shares after sync" "$SERVER_URL/api/shares" "${auth_header[@]}"

echo "== share descriptor snapshot =="
curl -fsS "${auth_header[@]}" "$SERVER_URL/api/shares" | node -e '
let s=""; process.stdin.on("data", d => s += d); process.stdin.on("end", () => {
  const data = JSON.parse(s);
  const shares = data.shares || [];
  console.log(JSON.stringify(shares.map((share) => ({
    id: share.id,
    app: share.app,
    providerId: share.providerId,
    providerType: share.providerType,
    accountEmail: share.accountEmail,
    subscriptionLevel: share.subscriptionLevel,
    quotaPercent: share.quotaPercent,
    marketGrant: share.marketGrant,
    runtimeSnapshot: share.runtimeSnapshot
  })), null, 2));
});'
echo

echo "== provider health =="
fetch_required "provider health" "$SERVER_URL/api/providers/health" "${auth_header[@]}"

echo "== recent usage logs =="
fetch_required "recent usage logs" "$SERVER_URL/api/usage/logs?limit=20" "${auth_header[@]}"

echo "== summary =="
echo "failures=${FAILURES} warnings=${WARNINGS}"
if [[ -n "$EVIDENCE_FILE" ]]; then
  EVIDENCE_STAGE="${EVIDENCE_STAGE:-router-market-smoke}" \
  EVIDENCE_STATUS="$([[ "$FAILURES" -eq 0 ]] && echo pass || echo fail)" \
  FAILURES="$FAILURES" WARNINGS="$WARNINGS" \
  LOCAL_SHARE_STATUS="$LOCAL_SHARE_STATUS" \
  DIRECT_NOAUTH_STATUS="$DIRECT_NOAUTH_STATUS" \
  DIRECT_PUBLIC_STATUS="$DIRECT_PUBLIC_STATUS" \
  DIRECT_PUBLIC_STREAM_STATUS="$DIRECT_PUBLIC_STREAM_STATUS" \
  MARKET_API_STATUS="$MARKET_API_STATUS" \
  MARKET_API_STREAM_STATUS="$MARKET_API_STREAM_STATUS" \
  MARKET_HEALTH_STATUS="$MARKET_HEALTH_STATUS" \
    node scripts/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
fi
if [[ "$FAILURES" -gt 0 ]]; then
  exit 1
fi
