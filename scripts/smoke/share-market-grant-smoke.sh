#!/usr/bin/env bash
set -euo pipefail

# Real share-market grant add/revoke smoke.
#
# Required:
#   SERVER_URL
#   CC_SWITCH_SERVER_TOKEN
#   SHARE_ID
#   SHARE_MARKET_GRANT_TOKEN
#   SHARE_MARKET_BUYER_EMAIL
#   SHARE_MARKET_LISTING_ID
#   SHARE_MARKET_ORDER_ID
#
# Optional:
#   ROUTER_BASE_URL                         Default: https://jptokenswitch.cc
#   SHARE_MARKET_GRANT_TOKEN_HEADER         Default: Authorization
#   SHARE_MARKET_APP_TYPE                   Default: codex
#   SHARE_MARKET_GRANT_ID_PREFIX            Default generated from timestamp
#   SHARE_MARKET_GRANT_MODE                 Default: add-revoke (add-revoke|add-only|revoke-only|noop-only|rejected-only)
#   SHARE_MARKET_GRANT_EXPECTED_STATUS      Optional final status assertion (applied|noop|rejected|unknown)
#   SHARE_MARKET_GRANT_REJECT_ACTION        Default: invalid; only used by rejected-only.
#   SHARE_MARKET_GRANT_POLL_SECONDS         Default: 30
#   SHARE_MARKET_GRANT_POLL_INTERVAL_SECONDS Default: 2
#   SHARE_MARKET_GRANT_POLL_ATTEMPTS        Optional; overrides seconds-derived attempts.
#   SHARE_MARKET_SKIP_BUYER_VISIBILITY      Default: 0
#   EVIDENCE_FILE                           Optional redacted evidence output.

SERVER_URL="${SERVER_URL:-http://127.0.0.1:15721}"
API_TOKEN="${CC_SWITCH_SERVER_TOKEN:-}"
ROUTER_BASE_URL="${ROUTER_BASE_URL:-https://jptokenswitch.cc}"
SHARE_MARKET_URL="${SHARE_MARKET_URL:-}"
SHARE_ID="${SHARE_ID:-}"
GRANT_TOKEN="${SHARE_MARKET_GRANT_TOKEN:-}"
GRANT_TOKEN_HEADER="${SHARE_MARKET_GRANT_TOKEN_HEADER:-Authorization}"
BUYER_EMAIL="${SHARE_MARKET_BUYER_EMAIL:-}"
LISTING_ID="${SHARE_MARKET_LISTING_ID:-}"
ORDER_ID="${SHARE_MARKET_ORDER_ID:-}"
APP_TYPE="${SHARE_MARKET_APP_TYPE:-codex}"
GRANT_ID_PREFIX="${SHARE_MARKET_GRANT_ID_PREFIX:-ac8-$(date -u +%Y%m%dT%H%M%SZ)}"
GRANT_MODE="${SHARE_MARKET_GRANT_MODE:-add-revoke}"
EXPECTED_STATUS="${SHARE_MARKET_GRANT_EXPECTED_STATUS:-}"
REJECT_ACTION="${SHARE_MARKET_GRANT_REJECT_ACTION:-invalid}"
POLL_SECONDS="${SHARE_MARKET_GRANT_POLL_SECONDS:-30}"
POLL_INTERVAL_SECONDS="${SHARE_MARKET_GRANT_POLL_INTERVAL_SECONDS:-2}"
POLL_ATTEMPTS="${SHARE_MARKET_GRANT_POLL_ATTEMPTS:-}"
SKIP_BUYER_VISIBILITY="${SHARE_MARKET_SKIP_BUYER_VISIBILITY:-0}"
EVIDENCE_FILE="${EVIDENCE_FILE:-}"
FAILURES=0
WARNINGS=0
BLOCKED=0
MISSING_VARS=()
SHARE_MARKET_ADD_STATUS=""
SHARE_MARKET_REVOKE_STATUS=""
SHARE_MARKET_ADD_EDIT_ID=""
SHARE_MARKET_REVOKE_EDIT_ID=""

pass() { echo "[PASS] $*"; }
warn() { WARNINGS=$((WARNINGS + 1)); echo "[WARN] $*"; }
fail() { FAILURES=$((FAILURES + 1)); echo "[FAIL] $*"; }
block() { BLOCKED=$((BLOCKED + 1)); echo "[BLOCKED] $*"; }

is_set() {
  local name="$1"
  local value="${!name:-}"
  [[ -n "$value" && "$value" != \<* ]]
}

require_named_var() {
  local name="$1"
  local public_name="$2"
  if ! is_set "$name"; then
    MISSING_VARS+=("$public_name")
    block "$public_name is required"
  fi
}

json_get() {
  local expr="$1"
  node -e "
let s = '';
process.stdin.on('data', d => s += d);
process.stdin.on('end', () => {
  const data = JSON.parse(s);
  const value = ${expr};
  if (value === undefined || value === null) process.exit(1);
  process.stdout.write(String(value));
});
"
}

redact_email() {
  node -e '
const value = process.argv[1] || "";
if (!value.includes("@")) {
  process.stdout.write(value);
  process.exit(0);
}
const [name, domain] = value.split("@");
process.stdout.write(`${name.slice(0, 2)}${"*".repeat(Math.max(1, name.length - 2))}@${domain}`);
' "$1"
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

validate_mode() {
  case "$GRANT_MODE" in
    add-revoke|add-only|revoke-only|noop-only|rejected-only) ;;
    *)
      echo "unsupported SHARE_MARKET_GRANT_MODE: $GRANT_MODE" >&2
      exit 2
      ;;
  esac
  if [[ -n "$EXPECTED_STATUS" ]]; then
    case "$EXPECTED_STATUS" in
      applied|noop|rejected|unknown|error) ;;
      *)
        echo "unsupported SHARE_MARKET_GRANT_EXPECTED_STATUS: $EXPECTED_STATUS" >&2
        exit 2
        ;;
    esac
  fi
  if [[ "$POLL_INTERVAL_SECONDS" -lt 1 ]]; then
    echo "SHARE_MARKET_GRANT_POLL_INTERVAL_SECONDS must be >= 1" >&2
    exit 2
  fi
}

should_run_add() {
  [[ "$GRANT_MODE" == "add-revoke" || "$GRANT_MODE" == "add-only" || "$GRANT_MODE" == "noop-only" || "$GRANT_MODE" == "rejected-only" ]]
}

should_run_revoke() {
  [[ "$GRANT_MODE" == "add-revoke" || "$GRANT_MODE" == "revoke-only" ]]
}

add_action_for_mode() {
  if [[ "$GRANT_MODE" == "rejected-only" ]]; then
    printf '%s' "$REJECT_ACTION"
  else
    printf 'add'
  fi
}

check_expected_status() {
  local label="$1"
  local status="$2"
  if [[ -z "$EXPECTED_STATUS" ]]; then
    return
  fi
  if [[ "$status" == "$EXPECTED_STATUS" ]]; then
    pass "$label final status matched expected ${EXPECTED_STATUS}"
  else
    fail "$label final status expected ${EXPECTED_STATUS}, got ${status:-unknown}"
  fi
}

grant_auth_header=()
case "$GRANT_MODE" in
  noop-only)
    EXPECTED_STATUS="${EXPECTED_STATUS:-noop}"
    ;;
  rejected-only)
    EXPECTED_STATUS="${EXPECTED_STATUS:-rejected}"
    ;;
esac
validate_mode
case "$GRANT_TOKEN_HEADER" in
  Authorization|authorization)
    grant_auth_header=(-H "Authorization: Bearer $GRANT_TOKEN")
    ;;
  x-api-key|X-API-Key|x-goog-api-key|X-Goog-Api-Key)
    grant_auth_header=(-H "$GRANT_TOKEN_HEADER: $GRANT_TOKEN")
    ;;
  *)
    echo "unsupported SHARE_MARKET_GRANT_TOKEN_HEADER: $GRANT_TOKEN_HEADER" >&2
    exit 2
    ;;
esac

server_auth_header=(-H "Authorization: Bearer $API_TOKEN")

grant_payload() {
  local action="$1"
  local grant_id="$2"
  ACTION="$action" GRANT_ID="$grant_id" APP_TYPE="$APP_TYPE" BUYER_EMAIL="$BUYER_EMAIL" \
  LISTING_ID="$LISTING_ID" ORDER_ID="$ORDER_ID" node -e '
const payload = {
  grantId: process.env.GRANT_ID,
  action: process.env.ACTION,
  appType: process.env.APP_TYPE || undefined,
  buyerEmails: [process.env.BUYER_EMAIL],
  orderIds: [process.env.ORDER_ID],
  listingId: process.env.LISTING_ID
};
process.stdout.write(JSON.stringify(payload));
'
}

post_grant() {
  local action="$1"
  local grant_id="$2"
  local out status
  out="$(mktemp /tmp/cc-switch-server-grant.XXXXXX)"
  status="$(curl -LsS -o "$out" -w "%{http_code}" \
    -X POST \
    -H "Content-Type: application/json" \
    "${grant_auth_header[@]}" \
    -d "$(grant_payload "$action" "$grant_id")" \
    "${ROUTER_BASE_URL%/}/v1/share-market/shares/${SHARE_ID}/grants" || true)"
  echo "${action} grant status=${status}" >&2
  node -e '
const fs = require("fs");
try {
  const data = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  console.error(JSON.stringify({
    ok: data.ok,
    grantId: data.grantId,
    routerEditId: data.routerEditId,
    status: data.status,
    error: data.error || data.errorMessage || data.message
  }, null, 2));
} catch {
  const text = fs.readFileSync(process.argv[1], "utf8");
  console.error(text.slice(0, 800));
}
' "$out"
  if [[ "$status" =~ ^2 ]] && ! json_ok_false "$out"; then
    cat "$out"
    rm -f "$out"
    return 0
  fi
  rm -f "$out"
  return 1
}

pull_edits() {
  local out status
  out="$(mktemp /tmp/cc-switch-server-grant-pull.XXXXXX)"
  status="$(curl -LsS -o "$out" -w "%{http_code}" \
    -X POST \
    "${server_auth_header[@]}" \
    "${SERVER_URL%/}/api/router/share-edits/pull" || true)"
  echo "server share edit pull status=${status}"
  cat "$out"
  echo
  if [[ "$status" =~ ^2 ]] && ! json_ok_false "$out"; then
    rm -f "$out"
    return 0
  fi
  rm -f "$out"
  return 1
}

grant_status() {
  local edit_id="$1"
  local out status
  out="$(mktemp /tmp/cc-switch-server-grant-status.XXXXXX)"
  status="$(curl -LsS -o "$out" -w "%{http_code}" \
    "${grant_auth_header[@]}" \
    "${ROUTER_BASE_URL%/}/v1/share-market/shares/${SHARE_ID}/grants/${edit_id}" || true)"
  if [[ "$status" =~ ^2 ]] && ! json_ok_false "$out"; then
    cat "$out"
    rm -f "$out"
    return 0
  fi
  echo "grant status query failed status=${status}" >&2
  cat "$out" >&2
  echo >&2
  rm -f "$out"
  return 1
}

poll_grant_status() {
  local label="$1"
  local edit_id="$2"
  local max_attempts response current_status attempt
  if [[ -n "$POLL_ATTEMPTS" ]]; then
    max_attempts="$POLL_ATTEMPTS"
  else
    max_attempts=$(((POLL_SECONDS + POLL_INTERVAL_SECONDS - 1) / POLL_INTERVAL_SECONDS))
    if [[ "$max_attempts" -lt 1 ]]; then
      max_attempts=1
    fi
  fi
  current_status=""
  for ((attempt = 1; attempt <= max_attempts; attempt++)); do
    response="$(grant_status "$edit_id" || true)"
    if [[ -n "$response" ]]; then
      current_status="$(printf '%s' "$response" | json_get 'data.status' || true)"
      echo "${label} grant routerEditId=${edit_id} status=${current_status} attempt=${attempt}/${max_attempts}" >&2
      if [[ "$current_status" == "applied" || "$current_status" == "rejected" || "$current_status" == "noop" ]]; then
        printf '%s' "$current_status"
        return 0
      fi
    fi
    if [[ "$attempt" -lt "$max_attempts" ]]; then
      sleep "$POLL_INTERVAL_SECONDS"
      pull_edits >/dev/null || true
    fi
  done
  printf '%s' "${current_status:-unknown}"
  return 0
}

check_buyer_visibility() {
  local expected="$1"
  local out status result
  out="$(mktemp /tmp/cc-switch-server-grant-shares.XXXXXX)"
  status="$(curl -LsS -o "$out" -w "%{http_code}" \
    "${server_auth_header[@]}" \
    "${SERVER_URL%/}/api/shares" || true)"
  if [[ ! "$status" =~ ^2 ]]; then
    warn "could not fetch server shares after grant"
    rm -f "$out"
    return
  fi
  result="$(APP_TYPE="$APP_TYPE" SHARE_ID="$SHARE_ID" BUYER_EMAIL="$BUYER_EMAIL" node -e '
const fs = require("fs");
const data = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const share = (data.shares || []).find((item) => item.id === process.env.SHARE_ID);
if (!share) {
  process.stdout.write("missing_share");
  process.exit(0);
}
const buyer = (process.env.BUYER_EMAIL || "").trim().toLowerCase();
const app = process.env.APP_TYPE || "";
const acl = (share.acl && share.acl.sharedWithEmails) || [];
const appEmails = (((share.appSettings || {})[app] || {}).sharedWithEmails) || [];
const all = [...acl, ...appEmails].map((value) => String(value).trim().toLowerCase());
process.stdout.write(all.includes(buyer) ? "present" : "absent");
' "$out")"
  rm -f "$out"
  if [[ "$result" == "$expected" ]]; then
    pass "buyer email is ${expected} in server share state"
  else
    warn "buyer email state expected ${expected}, got ${result}"
  fi
}

echo "== share-market grant smoke =="
echo "routerBaseUrl=${ROUTER_BASE_URL}"
echo "shareMarketUrl=${SHARE_MARKET_URL:-<not-set>}"
echo "serverUrl=${SERVER_URL}"
echo "shareId=${SHARE_ID:-<missing>}"
echo "buyer=$(redact_email "$BUYER_EMAIL")"
echo "appType=${APP_TYPE}"
echo "grantMode=${GRANT_MODE}"
echo "expectedStatus=${EXPECTED_STATUS:-<not-set>}"
echo "pollSeconds=${POLL_SECONDS}"
echo "pollIntervalSeconds=${POLL_INTERVAL_SECONDS}"
echo "pollAttempts=${POLL_ATTEMPTS:-<derived>}"
echo "skipBuyerVisibility=${SKIP_BUYER_VISIBILITY}"
echo "No token values are printed."

require_named_var API_TOKEN CC_SWITCH_SERVER_TOKEN
require_named_var SHARE_ID SHARE_ID
require_named_var GRANT_TOKEN SHARE_MARKET_GRANT_TOKEN
require_named_var BUYER_EMAIL SHARE_MARKET_BUYER_EMAIL
require_named_var LISTING_ID SHARE_MARKET_LISTING_ID
require_named_var ORDER_ID SHARE_MARKET_ORDER_ID
if [[ "$BLOCKED" -gt 0 ]]; then
  if [[ -n "$EVIDENCE_FILE" ]]; then
    SHARE_MARKET_ADD_STATUS="blocked" SHARE_MARKET_REVOKE_STATUS="blocked" \
    BLOCKED_GROUPS="$BLOCKED" FAILURES="$FAILURES" WARNINGS="$WARNINGS" \
    EVIDENCE_TARGET="share-market-grant" BLOCKER_GROUP="missing-grant-token" \
    EVIDENCE_STATUS="blocked" EVIDENCE_NOTES="share-market grant inputs are incomplete: ${MISSING_VARS[*]}" \
      node scripts/smoke/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
  fi
  echo "blocked=${BLOCKED}"
  exit 0
fi

add_grant_id="${GRANT_ID_PREFIX}-add"
if [[ "$GRANT_MODE" == "noop-only" ]]; then
  add_grant_id="${GRANT_ID_PREFIX}-noop"
elif [[ "$GRANT_MODE" == "rejected-only" ]]; then
  add_grant_id="${GRANT_ID_PREFIX}-rejected"
fi
revoke_grant_id="${GRANT_ID_PREFIX}-revoke"

if should_run_add; then
  echo "== add grant =="
  add_action="$(add_action_for_mode)"
  add_response="$(post_grant "$add_action" "$add_grant_id" || true)"
  printf '%s\n' "$add_response"
  if [[ -z "$add_response" ]]; then
    fail "${add_action} grant request failed"
  else
    SHARE_MARKET_ADD_STATUS="$(printf '%s' "$add_response" | json_get 'data.status' || true)"
    SHARE_MARKET_ADD_EDIT_ID="$(printf '%s' "$add_response" | json_get 'data.routerEditId' || true)"
    if [[ "$SHARE_MARKET_ADD_STATUS" == "pending" ]]; then
      pass "${add_action} grant created pending edit"
      pull_edits || warn "server pull after add grant did not complete"
      SHARE_MARKET_ADD_STATUS="$(poll_grant_status add "$SHARE_MARKET_ADD_EDIT_ID")"
      if [[ "$SHARE_MARKET_ADD_STATUS" == "applied" || "$SHARE_MARKET_ADD_STATUS" == "noop" ]]; then
        pass "${add_action} grant reached ${SHARE_MARKET_ADD_STATUS}"
      else
        warn "${add_action} grant final status ${SHARE_MARKET_ADD_STATUS}"
      fi
    elif [[ "$SHARE_MARKET_ADD_STATUS" == "noop" ]]; then
      pass "${add_action} grant was noop"
    elif [[ "$SHARE_MARKET_ADD_STATUS" == "rejected" || "$SHARE_MARKET_ADD_STATUS" == "error" ]]; then
      if [[ "$GRANT_MODE" == "rejected-only" ]]; then
        pass "${add_action} grant reached ${SHARE_MARKET_ADD_STATUS}"
      else
        warn "${add_action} grant status ${SHARE_MARKET_ADD_STATUS}"
      fi
    else
      warn "${add_action} grant status ${SHARE_MARKET_ADD_STATUS:-unknown}"
    fi
    check_expected_status "$add_action" "$SHARE_MARKET_ADD_STATUS"
  fi
  if [[ "$GRANT_MODE" == "rejected-only" ]]; then
    warn "buyer visibility check skipped for rejected-only grant"
  elif [[ "$SKIP_BUYER_VISIBILITY" == "1" ]]; then
    warn "buyer visibility check skipped after add grant"
  else
    check_buyer_visibility present
  fi
else
  SHARE_MARKET_ADD_STATUS="skipped"
  echo "[SKIP] add grant disabled by SHARE_MARKET_GRANT_MODE=${GRANT_MODE}"
fi

if should_run_revoke; then
  echo "== revoke grant =="
  revoke_response="$(post_grant revoke "$revoke_grant_id" || true)"
  printf '%s\n' "$revoke_response"
  if [[ -z "$revoke_response" ]]; then
    fail "revoke grant request failed"
  else
    SHARE_MARKET_REVOKE_STATUS="$(printf '%s' "$revoke_response" | json_get 'data.status' || true)"
    SHARE_MARKET_REVOKE_EDIT_ID="$(printf '%s' "$revoke_response" | json_get 'data.routerEditId' || true)"
    if [[ "$SHARE_MARKET_REVOKE_STATUS" == "pending" ]]; then
      pass "revoke grant created pending edit"
      pull_edits || warn "server pull after revoke grant did not complete"
      SHARE_MARKET_REVOKE_STATUS="$(poll_grant_status revoke "$SHARE_MARKET_REVOKE_EDIT_ID")"
      if [[ "$SHARE_MARKET_REVOKE_STATUS" == "applied" || "$SHARE_MARKET_REVOKE_STATUS" == "noop" ]]; then
        pass "revoke grant reached ${SHARE_MARKET_REVOKE_STATUS}"
      else
        warn "revoke grant final status ${SHARE_MARKET_REVOKE_STATUS}"
      fi
    elif [[ "$SHARE_MARKET_REVOKE_STATUS" == "noop" ]]; then
      pass "revoke grant was noop"
    else
      warn "revoke grant status ${SHARE_MARKET_REVOKE_STATUS:-unknown}"
    fi
    check_expected_status revoke "$SHARE_MARKET_REVOKE_STATUS"
  fi
  if [[ "$SKIP_BUYER_VISIBILITY" == "1" ]]; then
    warn "buyer visibility check skipped after revoke grant"
  else
    check_buyer_visibility absent
  fi
else
  SHARE_MARKET_REVOKE_STATUS="skipped"
  echo "[SKIP] revoke grant disabled by SHARE_MARKET_GRANT_MODE=${GRANT_MODE}"
fi

echo "== summary =="
echo "failures=${FAILURES} warnings=${WARNINGS} blocked=${BLOCKED}"
echo "addStatus=${SHARE_MARKET_ADD_STATUS:-unknown}"
echo "revokeStatus=${SHARE_MARKET_REVOKE_STATUS:-unknown}"

if [[ -n "$EVIDENCE_FILE" ]]; then
  SHARE_MARKET_ADD_STATUS="$SHARE_MARKET_ADD_STATUS" \
  SHARE_MARKET_REVOKE_STATUS="$SHARE_MARKET_REVOKE_STATUS" \
  SHARE_MARKET_ADD_EDIT_ID="$SHARE_MARKET_ADD_EDIT_ID" \
  SHARE_MARKET_REVOKE_EDIT_ID="$SHARE_MARKET_REVOKE_EDIT_ID" \
  FAILURES="$FAILURES" WARNINGS="$WARNINGS" BLOCKED_GROUPS="$BLOCKED" \
  EVIDENCE_TARGET="share-market-grant" \
  BLOCKER_GROUP="$([[ "$BLOCKED" -gt 0 ]] && echo missing-grant-token || echo "")" \
  FAILURE_CLASS="$([[ "$FAILURES" -gt 0 ]] && echo grant-ack || echo "")" \
  EVIDENCE_STATUS="$([[ "$FAILURES" -eq 0 && "$BLOCKED" -eq 0 ]] && echo pass || echo fail)" \
  EVIDENCE_NOTES="mode=${GRANT_MODE}; expectedStatus=${EXPECTED_STATUS:-none}; pollInterval=${POLL_INTERVAL_SECONDS}; pollAttempts=${POLL_ATTEMPTS:-derived}; skipBuyerVisibility=${SKIP_BUYER_VISIBILITY}" \
    node scripts/smoke/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
fi

if [[ "$FAILURES" -gt 0 ]]; then
  exit 1
fi
