#!/usr/bin/env bash
set -euo pipefail

SERVER_URL="${SERVER_URL:-http://127.0.0.1:15721}"
API_TOKEN="${CC_SWITCH_SERVER_TOKEN:-}"
DIRECT_SHARE_URL="${DIRECT_SHARE_URL:-}"
DIRECT_CLAUDE_SHARE_URL="${DIRECT_CLAUDE_SHARE_URL:-}"
DIRECT_CODEX_SHARE_URL="${DIRECT_CODEX_SHARE_URL:-${DIRECT_SHARE_URL}}"
DIRECT_GEMINI_SHARE_URL="${DIRECT_GEMINI_SHARE_URL:-}"
MARKET_API_URL="${MARKET_API_URL:-}"
MARKET_CLAUDE_API_URL="${MARKET_CLAUDE_API_URL:-}"
MARKET_CODEX_API_URL="${MARKET_CODEX_API_URL:-${MARKET_API_URL}}"
MARKET_GEMINI_API_URL="${MARKET_GEMINI_API_URL:-}"
ROUTER_API_TOKEN="${ROUTER_API_TOKEN:-}"
ROUTER_API_TOKEN_HEADER="${ROUTER_API_TOKEN_HEADER:-Authorization}"
MARKET_API_TOKEN="${MARKET_API_TOKEN:-}"
MARKET_API_TOKEN_HEADER="${MARKET_API_TOKEN_HEADER:-}"
PROBE_MODEL="${PROBE_MODEL:-probe}"
RUN_PROBES="${RUN_PROBES:-0}"
STREAM_PROBE="${STREAM_PROBE:-0}"
REQUIRE_STREAM_USAGE="${REQUIRE_STREAM_USAGE:-0}"
EVIDENCE_FILE="${EVIDENCE_FILE:-}"
FAILURES=0
WARNINGS=0
BLOCKED=0
BLOCKERS=()

SERVER_HEALTH_STATUS=""
ROUTER_STATUS_STATUS=""
ROUTER_DIAGNOSTICS_STATUS=""
ROUTER_TUNNELS_STATUS=""
SHARES_STATUS=""
USAGE_LOGS_STATUS=""
PROVIDER_HEALTH_STATUS=""
DIRECT_NOAUTH_STATUS=""
DIRECT_PUBLIC_STATUS=""
DIRECT_PUBLIC_STREAM_STATUS=""
DIRECT_CLAUDE_STATUS=""
DIRECT_CODEX_STATUS=""
DIRECT_GEMINI_STATUS=""
DIRECT_CLAUDE_STREAM_STATUS=""
DIRECT_CODEX_STREAM_STATUS=""
DIRECT_GEMINI_STREAM_STATUS=""
MARKET_API_STATUS=""
MARKET_API_STREAM_STATUS=""
MARKET_CLAUDE_STATUS=""
MARKET_CODEX_STATUS=""
MARKET_GEMINI_STATUS=""
MARKET_CLAUDE_STREAM_STATUS=""
MARKET_CODEX_STREAM_STATUS=""
MARKET_GEMINI_STREAM_STATUS=""
DIAGNOSTICS_CLASSIFICATION=""

pass() { echo "[PASS] $*"; }
warn() { WARNINGS=$((WARNINGS + 1)); echo "[WARN] $*"; }
fail() { FAILURES=$((FAILURES + 1)); echo "[FAIL] $*"; }
block() { BLOCKED=$((BLOCKED + 1)); BLOCKERS+=("$*"); echo "[BLOCKED] $*"; }

auth_header=()
if [[ -n "$API_TOKEN" && "$API_TOKEN" != \<* ]]; then
  auth_header=(-H "Authorization: Bearer $API_TOKEN")
fi

build_token_header() {
  local token="$1"
  local header="$2"
  if [[ -z "$token" || "$token" == \<* ]]; then
    return 1
  fi
  case "$header" in
    Authorization|authorization) printf '%s\n%s\n' "-H" "Authorization: Bearer $token" ;;
    x-api-key|X-API-Key|x-goog-api-key|X-Goog-Api-Key) printf '%s\n%s\n' "-H" "$header: $token" ;;
    *) return 2 ;;
  esac
}

router_auth_header=()
if header_lines="$(build_token_header "$ROUTER_API_TOKEN" "$ROUTER_API_TOKEN_HEADER" 2>/dev/null)"; then
  mapfile -t router_auth_header <<< "$header_lines"
elif [[ -n "$ROUTER_API_TOKEN" && "$ROUTER_API_TOKEN" != \<* ]]; then
  echo "unsupported ROUTER_API_TOKEN_HEADER: $ROUTER_API_TOKEN_HEADER" >&2
  exit 2
fi

market_auth_header=("${router_auth_header[@]}")
if [[ -n "$MARKET_API_TOKEN" && "$MARKET_API_TOKEN" != \<* ]]; then
  market_header="${MARKET_API_TOKEN_HEADER:-Authorization}"
  if header_lines="$(build_token_header "$MARKET_API_TOKEN" "$market_header" 2>/dev/null)"; then
    mapfile -t market_auth_header <<< "$header_lines"
  else
    echo "unsupported MARKET_API_TOKEN_HEADER: $market_header" >&2
    exit 2
  fi
fi

summarize_json_file() {
  local file="$1"
  node -e '
const fs = require("fs");
const file = process.argv[1];
let data;
try {
  data = JSON.parse(fs.readFileSync(file, "utf8"));
} catch {
  const text = fs.readFileSync(file, "utf8");
  console.log(JSON.stringify({parse: "text", preview: text.slice(0, 300)}, null, 2));
  process.exit(0);
}
function pick(value) {
  if (!value || typeof value !== "object") return value;
  const keys = [
    "ok", "error", "message", "status", "registered", "routerUrl",
    "installationId", "clientTunnel", "shareTunnels", "pulled", "applied",
    "rejected", "acked", "ackFailed", "remoteSynced", "remoteSyncFailed"
  ];
  const out = {};
  for (const key of keys) {
    if (value[key] !== undefined) out[key] = value[key];
  }
  if (Array.isArray(value.shares)) {
    out.shares = value.shares.slice(0, 10).map((share) => ({
      id: share.id,
      app: share.app,
      providerId: share.providerId,
      providerType: share.providerType,
      status: share.status,
      routerUrl: share.routerUrl,
      marketGrant: share.marketGrant,
      runtimeSnapshot: share.runtimeSnapshot && {
        health: share.runtimeSnapshot.health,
        providerType: share.runtimeSnapshot.providerType,
        quotaPercent: share.runtimeSnapshot.quotaPercent,
        marketGrant: share.runtimeSnapshot.marketGrant
      }
    }));
  }
  if (Array.isArray(value.logs)) {
    out.logs = value.logs.slice(0, 10).map((log) => ({
      requestId: log.requestId,
      shareId: log.shareId,
      app: log.app,
      providerId: log.providerId,
      dataSource: log.dataSource,
      requestedModel: log.requestedModel,
      actualModel: log.actualModel,
      pricingModel: log.pricingModel,
      statusCode: log.statusCode,
      inputTokens: log.inputTokens,
      outputTokens: log.outputTokens,
      totalTokens: log.totalTokens
    }));
  }
  if (Array.isArray(value.providers)) {
    out.providers = value.providers.slice(0, 10).map((provider) => ({
      id: provider.id,
      app: provider.app,
      providerType: provider.providerType,
      healthy: provider.healthy,
      reason: provider.reason
    }));
  }
  console.log(JSON.stringify(Object.keys(out).length ? out : value, null, 2));
}
pick(data);
' "$file"
}

fetch_summary() {
  local label="$1"
  local url="$2"
  shift 2
  local out status
  out="$(mktemp /tmp/cc-switch-server-diagnostics.XXXXXX)"
  status="$(curl -LsS --max-time 20 -o "$out" -w "%{http_code}" "$@" "$url" || true)"
  echo "== ${label} =="
  echo "status=${status}"
  summarize_json_file "$out"
  rm -f "$out"
  echo
  if [[ "$status" =~ ^2 ]]; then
    pass "$label"
  else
    warn "$label"
  fi
  FETCH_STATUS="$status"
}

probe_payload() {
  local app="$1"
  APP="$app" node -e '
const model = process.env.PROBE_MODEL || "probe";
switch (process.env.APP) {
  case "claude":
    process.stdout.write(JSON.stringify({
      model,
      max_tokens: 1,
      messages: [{role: "user", content: "ping"}],
      stream: false
    }));
    break;
  case "gemini":
    process.stdout.write(JSON.stringify({
      contents: [{role: "user", parts: [{text: "ping"}]}],
      generationConfig: {maxOutputTokens: 1}
    }));
    break;
  default:
    process.stdout.write(JSON.stringify({
      model,
      input: "ping",
      stream: false,
      max_output_tokens: 1
    }));
}
'
}

stream_probe_payload() {
  local app="$1"
  APP="$app" node -e '
const model = process.env.PROBE_MODEL || "probe";
switch (process.env.APP) {
  case "claude":
    process.stdout.write(JSON.stringify({
      model,
      max_tokens: 1,
      messages: [{role: "user", content: "stream ping"}],
      stream: true
    }));
    break;
  case "gemini":
    process.stdout.write(JSON.stringify({
      contents: [{role: "user", parts: [{text: "stream ping"}]}],
      generationConfig: {maxOutputTokens: 1}
    }));
    break;
  default:
    process.stdout.write(JSON.stringify({
      model,
      input: "stream ping",
      stream: true,
      max_output_tokens: 1
    }));
}
'
}

probe_path() {
  local app="$1"
  local stream="${2:-0}"
  case "$app" in
    claude) printf '/v1/messages' ;;
    gemini)
      if [[ "$stream" == "1" ]]; then
        printf '/v1beta/models/%s:streamGenerateContent' "$PROBE_MODEL"
      else
        printf '/v1beta/models/%s:generateContent' "$PROBE_MODEL"
      fi
      ;;
    *) printf '/v1/responses' ;;
  esac
}

probe_url() {
  local base="$1"
  local app="$2"
  local stream="${3:-0}"
  printf '%s%s' "${base%/}" "$(probe_path "$app" "$stream")"
}

post_probe_summary() {
  local label="$1"
  local app="$2"
  local base_url="$3"
  shift 3
  local url
  url="$(probe_url "$base_url" "$app")"
  local out status
  out="$(mktemp /tmp/cc-switch-server-diagnostics-probe.XXXXXX)"
  status="$(curl -LsS --max-time 60 -o "$out" -w "%{http_code}" \
    -H "Content-Type: application/json" "$@" -d "$(probe_payload "$app")" "$url" || true)"
  echo "== ${label} =="
  echo "url=${url}"
  echo "status=${status}"
  summarize_json_file "$out"
  rm -f "$out"
  echo
  if [[ "$status" =~ ^2 ]]; then
    pass "$label"
  else
    warn "$label"
  fi
  PROBE_STATUS="$status"
}

stream_probe_summary() {
  local label="$1"
  local app="$2"
  local base_url="$3"
  shift 3
  local url args summary_file ok status
  url="$(probe_url "$base_url" "$app" 1)"
  args=(--url "$url" --body "$(stream_probe_payload "$app")" --require-done)
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

  summary_file="$(mktemp /tmp/cc-switch-server-diagnostics-stream.XXXXXX)"
  echo "== ${label} =="
  echo "url=${url}"
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
  STREAM_STATUS="$status"
}

echo "== direct/market diagnostics =="
echo "serverUrl=${SERVER_URL}"
echo "directShareUrl=${DIRECT_SHARE_URL:-<not-set>}"
echo "directClaudeShareUrl=${DIRECT_CLAUDE_SHARE_URL:-<not-set>}"
echo "directCodexShareUrl=${DIRECT_CODEX_SHARE_URL:-<not-set>}"
echo "directGeminiShareUrl=${DIRECT_GEMINI_SHARE_URL:-<not-set>}"
echo "marketApiUrl=${MARKET_API_URL:-<not-set>}"
echo "marketClaudeApiUrl=${MARKET_CLAUDE_API_URL:-<not-set>}"
echo "marketCodexApiUrl=${MARKET_CODEX_API_URL:-<not-set>}"
echo "marketGeminiApiUrl=${MARKET_GEMINI_API_URL:-<not-set>}"
echo "runProbes=${RUN_PROBES}"
echo "streamProbe=${STREAM_PROBE}"
echo "No token values are printed."

fetch_summary "server health" "$SERVER_URL/health"
SERVER_HEALTH_STATUS="$FETCH_STATUS"

if [[ "${#auth_header[@]}" -eq 0 ]]; then
  block "CC_SWITCH_SERVER_TOKEN is required for authenticated server diagnostics"
else
  fetch_summary "router status" "$SERVER_URL/api/router/status" "${auth_header[@]}"
  ROUTER_STATUS_STATUS="$FETCH_STATUS"
  fetch_summary "router diagnostics" "$SERVER_URL/api/router/diagnostics" "${auth_header[@]}"
  ROUTER_DIAGNOSTICS_STATUS="$FETCH_STATUS"
  fetch_summary "router tunnels" "$SERVER_URL/api/router/tunnels" "${auth_header[@]}"
  ROUTER_TUNNELS_STATUS="$FETCH_STATUS"
  fetch_summary "shares" "$SERVER_URL/api/shares" "${auth_header[@]}"
  SHARES_STATUS="$FETCH_STATUS"
  fetch_summary "usage logs" "$SERVER_URL/api/usage/logs?limit=20" "${auth_header[@]}"
  USAGE_LOGS_STATUS="$FETCH_STATUS"
  fetch_summary "provider health" "$SERVER_URL/api/providers/health" "${auth_header[@]}"
  PROVIDER_HEALTH_STATUS="$FETCH_STATUS"
fi

if [[ "$RUN_PROBES" == "1" ]]; then
  if [[ -n "$DIRECT_CODEX_SHARE_URL" ]]; then
    post_probe_summary "direct codex unauthenticated probe" codex "$DIRECT_CODEX_SHARE_URL"
    DIRECT_NOAUTH_STATUS="$PROBE_STATUS"
    if [[ "${#router_auth_header[@]}" -gt 0 ]]; then
      post_probe_summary "direct codex authenticated probe" codex "$DIRECT_CODEX_SHARE_URL" "${router_auth_header[@]}"
      DIRECT_CODEX_STATUS="$PROBE_STATUS"
      DIRECT_PUBLIC_STATUS="$PROBE_STATUS"
      if [[ "$STREAM_PROBE" == "1" ]]; then
        stream_probe_summary "direct codex stream probe" codex "$DIRECT_CODEX_SHARE_URL" "${router_auth_header[@]}"
        DIRECT_CODEX_STREAM_STATUS="$STREAM_STATUS"
        DIRECT_PUBLIC_STREAM_STATUS="$STREAM_STATUS"
      fi
    else
      block "ROUTER_API_TOKEN is required for authenticated direct Codex probe"
    fi
  else
    warn "DIRECT_CODEX_SHARE_URL/DIRECT_SHARE_URL not set; skipped direct Codex probes"
  fi

  if [[ -n "$DIRECT_CLAUDE_SHARE_URL" ]]; then
    if [[ "${#router_auth_header[@]}" -gt 0 ]]; then
      post_probe_summary "direct claude authenticated probe" claude "$DIRECT_CLAUDE_SHARE_URL" "${router_auth_header[@]}"
      DIRECT_CLAUDE_STATUS="$PROBE_STATUS"
      if [[ "$STREAM_PROBE" == "1" ]]; then
        stream_probe_summary "direct claude stream probe" claude "$DIRECT_CLAUDE_SHARE_URL" "${router_auth_header[@]}"
        DIRECT_CLAUDE_STREAM_STATUS="$STREAM_STATUS"
      fi
    else
      block "ROUTER_API_TOKEN is required for direct Claude probe"
    fi
  else
    warn "DIRECT_CLAUDE_SHARE_URL not set; skipped direct Claude probe"
  fi

  if [[ -n "$DIRECT_GEMINI_SHARE_URL" ]]; then
    if [[ "${#router_auth_header[@]}" -gt 0 ]]; then
      post_probe_summary "direct gemini authenticated probe" gemini "$DIRECT_GEMINI_SHARE_URL" "${router_auth_header[@]}"
      DIRECT_GEMINI_STATUS="$PROBE_STATUS"
      if [[ "$STREAM_PROBE" == "1" ]]; then
        stream_probe_summary "direct gemini stream probe" gemini "$DIRECT_GEMINI_SHARE_URL" "${router_auth_header[@]}"
        DIRECT_GEMINI_STREAM_STATUS="$STREAM_STATUS"
      fi
    else
      block "ROUTER_API_TOKEN is required for direct Gemini probe"
    fi
  else
    warn "DIRECT_GEMINI_SHARE_URL not set; skipped direct Gemini probe"
  fi

  if [[ -n "$MARKET_CODEX_API_URL" ]]; then
    if [[ "${#market_auth_header[@]}" -gt 0 ]]; then
      post_probe_summary "market codex authenticated probe" codex "$MARKET_CODEX_API_URL" "${market_auth_header[@]}"
      MARKET_CODEX_STATUS="$PROBE_STATUS"
      MARKET_API_STATUS="$PROBE_STATUS"
      if [[ "$STREAM_PROBE" == "1" ]]; then
        stream_probe_summary "market codex stream probe" codex "$MARKET_CODEX_API_URL" "${market_auth_header[@]}"
        MARKET_CODEX_STREAM_STATUS="$STREAM_STATUS"
        MARKET_API_STREAM_STATUS="$STREAM_STATUS"
      fi
    else
      block "ROUTER_API_TOKEN or MARKET_API_TOKEN is required for market Codex probe"
    fi
  else
    warn "MARKET_CODEX_API_URL/MARKET_API_URL not set; skipped market Codex probe"
  fi

  if [[ -n "$MARKET_CLAUDE_API_URL" ]]; then
    if [[ "${#market_auth_header[@]}" -gt 0 ]]; then
      post_probe_summary "market claude authenticated probe" claude "$MARKET_CLAUDE_API_URL" "${market_auth_header[@]}"
      MARKET_CLAUDE_STATUS="$PROBE_STATUS"
      if [[ "$STREAM_PROBE" == "1" ]]; then
        stream_probe_summary "market claude stream probe" claude "$MARKET_CLAUDE_API_URL" "${market_auth_header[@]}"
        MARKET_CLAUDE_STREAM_STATUS="$STREAM_STATUS"
      fi
    else
      block "ROUTER_API_TOKEN or MARKET_API_TOKEN is required for market Claude probe"
    fi
  else
    warn "MARKET_CLAUDE_API_URL not set; skipped market Claude probe"
  fi

  if [[ -n "$MARKET_GEMINI_API_URL" ]]; then
    if [[ "${#market_auth_header[@]}" -gt 0 ]]; then
      post_probe_summary "market gemini authenticated probe" gemini "$MARKET_GEMINI_API_URL" "${market_auth_header[@]}"
      MARKET_GEMINI_STATUS="$PROBE_STATUS"
      if [[ "$STREAM_PROBE" == "1" ]]; then
        stream_probe_summary "market gemini stream probe" gemini "$MARKET_GEMINI_API_URL" "${market_auth_header[@]}"
        MARKET_GEMINI_STREAM_STATUS="$STREAM_STATUS"
      fi
    else
      block "ROUTER_API_TOKEN or MARKET_API_TOKEN is required for market Gemini probe"
    fi
  else
    warn "MARKET_GEMINI_API_URL not set; skipped market Gemini probe"
  fi
else
  warn "RUN_PROBES=0; direct/market provider probes skipped"
fi

if [[ "$BLOCKED" -gt 0 ]]; then
  DIAGNOSTICS_CLASSIFICATION="blocked-inputs"
elif [[ "$WARNINGS" -gt 0 ]]; then
  DIAGNOSTICS_CLASSIFICATION="diagnostic-warnings"
else
  DIAGNOSTICS_CLASSIFICATION="ready"
fi

if [[ "$BLOCKED" -gt 0 ]]; then
  if [[ -z "$API_TOKEN" || "$API_TOKEN" == \<* ]]; then
    BLOCKER_GROUP="missing-env"
  elif [[ -z "$ROUTER_API_TOKEN" || "$ROUTER_API_TOKEN" == \<* ]]; then
    BLOCKER_GROUP="missing-router-token"
  elif [[ -z "$MARKET_API_TOKEN" && -z "$MARKET_API_URL" && -z "$MARKET_CODEX_API_URL" ]]; then
    BLOCKER_GROUP="missing-market-auth"
  else
    BLOCKER_GROUP="external-readonly"
  fi
else
  BLOCKER_GROUP=""
fi

echo "== summary =="
echo "failures=${FAILURES} warnings=${WARNINGS} blocked=${BLOCKED}"
echo "classification=${DIAGNOSTICS_CLASSIFICATION}"

if [[ -n "$EVIDENCE_FILE" ]]; then
  SERVER_HEALTH_STATUS="$SERVER_HEALTH_STATUS" \
  ROUTER_STATUS_STATUS="$ROUTER_STATUS_STATUS" \
  ROUTER_DIAGNOSTICS_STATUS="$ROUTER_DIAGNOSTICS_STATUS" \
  ROUTER_TUNNELS_STATUS="$ROUTER_TUNNELS_STATUS" \
  SHARES_STATUS="$SHARES_STATUS" \
  USAGE_LOGS_STATUS="$USAGE_LOGS_STATUS" \
  PROVIDER_HEALTH_STATUS="$PROVIDER_HEALTH_STATUS" \
  DIRECT_NOAUTH_STATUS="$DIRECT_NOAUTH_STATUS" \
  DIRECT_PUBLIC_STATUS="$DIRECT_PUBLIC_STATUS" \
  DIRECT_PUBLIC_STREAM_STATUS="$DIRECT_PUBLIC_STREAM_STATUS" \
  DIRECT_CLAUDE_STATUS="$DIRECT_CLAUDE_STATUS" \
  DIRECT_CODEX_STATUS="$DIRECT_CODEX_STATUS" \
  DIRECT_GEMINI_STATUS="$DIRECT_GEMINI_STATUS" \
  DIRECT_CLAUDE_STREAM_STATUS="$DIRECT_CLAUDE_STREAM_STATUS" \
  DIRECT_CODEX_STREAM_STATUS="$DIRECT_CODEX_STREAM_STATUS" \
  DIRECT_GEMINI_STREAM_STATUS="$DIRECT_GEMINI_STREAM_STATUS" \
  MARKET_API_STATUS="$MARKET_API_STATUS" \
  MARKET_API_STREAM_STATUS="$MARKET_API_STREAM_STATUS" \
  MARKET_CLAUDE_STATUS="$MARKET_CLAUDE_STATUS" \
  MARKET_CODEX_STATUS="$MARKET_CODEX_STATUS" \
  MARKET_GEMINI_STATUS="$MARKET_GEMINI_STATUS" \
  MARKET_CLAUDE_STREAM_STATUS="$MARKET_CLAUDE_STREAM_STATUS" \
  MARKET_CODEX_STREAM_STATUS="$MARKET_CODEX_STREAM_STATUS" \
  MARKET_GEMINI_STREAM_STATUS="$MARKET_GEMINI_STREAM_STATUS" \
  DIAGNOSTICS_CLASSIFICATION="$DIAGNOSTICS_CLASSIFICATION" \
  BLOCKED_GROUPS="$BLOCKED" FAILURES="$FAILURES" WARNINGS="$WARNINGS" \
  BLOCKER_GROUP="$BLOCKER_GROUP" \
  FAILURE_CLASS="$([[ "$FAILURES" -gt 0 ]] && echo auth-or-lease || echo "")" \
  EVIDENCE_STATUS="$([[ "$FAILURES" -eq 0 && "$BLOCKED" -eq 0 ]] && echo pass || echo blocked)" \
  EVIDENCE_NOTES="blockers=${BLOCKERS[*]:-none}; RUN_PROBES=${RUN_PROBES}" \
    node scripts/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
fi

if [[ "$FAILURES" -gt 0 ]]; then
  exit 1
fi
