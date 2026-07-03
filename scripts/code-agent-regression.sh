#!/usr/bin/env bash
set -euo pipefail

SERVER_URL="${SERVER_URL:-http://127.0.0.1:15721}"
API_TOKEN="${CC_SWITCH_SERVER_TOKEN:-}"
SHARE_ID="${SHARE_ID:-}"
CLAUDE_SHARE_ID="${CLAUDE_SHARE_ID:-}"
CODEX_SHARE_ID="${CODEX_SHARE_ID:-${SHARE_ID}}"
GEMINI_SHARE_ID="${GEMINI_SHARE_ID:-}"
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
RUN_CONTRACT_TESTS="${RUN_CONTRACT_TESTS:-1}"
RUN_REAL="${RUN_REAL:-0}"
STREAM_PROBE="${STREAM_PROBE:-0}"
REQUIRE_STREAM_USAGE="${REQUIRE_STREAM_USAGE:-0}"
EVIDENCE_FILE="${EVIDENCE_FILE:-}"
MATRIX_PATH="${MATRIX_PATH:-docs/code-agent-regression-matrix.json}"
MATRIX_SUMMARY_FILE="${MATRIX_SUMMARY_FILE:-}"
FAILURES=0
WARNINGS=0
SKIPPED=0
MATRIX_TOTAL=0
MATRIX_RUNNABLE=0
MATRIX_SKIPPED=0
MATRIX_SKELETON=0

pass() { echo "[PASS] $*"; }
warn() { WARNINGS=$((WARNINGS + 1)); echo "[WARN] $*"; }
skip() { SKIPPED=$((SKIPPED + 1)); echo "[SKIP] $*"; }
fail() { FAILURES=$((FAILURES + 1)); echo "[FAIL] $*"; }

auth_header=()
if [[ -n "$API_TOKEN" ]]; then
  auth_header=(-H "Authorization: Bearer $API_TOKEN")
fi

router_auth_header=()
if [[ -n "$ROUTER_API_TOKEN" ]]; then
  case "$ROUTER_API_TOKEN_HEADER" in
    Authorization|authorization) router_auth_header=(-H "Authorization: Bearer $ROUTER_API_TOKEN") ;;
    x-api-key|X-API-Key|x-goog-api-key|X-Goog-Api-Key) router_auth_header=(-H "$ROUTER_API_TOKEN_HEADER: $ROUTER_API_TOKEN") ;;
    *) echo "unsupported ROUTER_API_TOKEN_HEADER: $ROUTER_API_TOKEN_HEADER" >&2; exit 2 ;;
  esac
fi

market_auth_header=("${router_auth_header[@]}")
if [[ -n "$MARKET_API_TOKEN" ]]; then
  market_header="${MARKET_API_TOKEN_HEADER:-Authorization}"
  case "$market_header" in
    Authorization|authorization) market_auth_header=(-H "Authorization: Bearer $MARKET_API_TOKEN") ;;
    x-api-key|X-API-Key|x-goog-api-key|X-Goog-Api-Key) market_auth_header=(-H "$market_header: $MARKET_API_TOKEN") ;;
    *) echo "unsupported MARKET_API_TOKEN_HEADER: $market_header" >&2; exit 2 ;;
  esac
fi

json_ok_false() {
  node -e '
const fs = require("fs");
try {
  const data = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  process.exit(data && data.ok === false ? 0 : 1);
} catch {
  process.exit(1);
}
' "$1"
}

read_matrix_field() {
  local field="$1"
  node -e '
const fs = require("fs");
const data = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const value = data[process.argv[2]];
process.stdout.write(value === undefined || value === null ? "" : String(value));
' "$MATRIX_SUMMARY_FILE" "$field"
}

probe() {
  local label="$1"
  local url="$2"
  local body="$3"
  shift 3
  local out status
  out="$(mktemp /tmp/cc-switch-server-regression.XXXXXX)"
  status="$(curl -LsS --max-time 60 -o "$out" -w "%{http_code}" \
    -H "Content-Type: application/json" "$@" -d "$body" "$url" || true)"
  echo "${label}: status=${status}"
  sed -n '1,12p' "$out"
  echo
  if [[ "$status" =~ ^2 ]] && ! json_ok_false "$out"; then
    pass "$label"
  elif [[ "$RUN_REAL" == "1" ]]; then
    fail "$label"
  else
    warn "$label returned non-2xx; treated as provider-level or fixture limitation"
  fi
  rm -f "$out"
}

stream_probe() {
  local label="$1"
  local url="$2"
  local body="$3"
  shift 3
  local args
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

  echo "${label}: stream"
  if node scripts/stream-probe.mjs "${args[@]}"; then
    pass "$label"
  elif [[ "$RUN_REAL" == "1" ]]; then
    fail "$label"
  else
    warn "$label returned non-passing stream summary; treated as provider-level or fixture limitation"
  fi
  echo
}

echo "== regression matrix =="
matrix_temp=""
if [[ -z "$MATRIX_SUMMARY_FILE" ]]; then
  if [[ -n "$EVIDENCE_FILE" ]]; then
    mkdir -p "$(dirname "$EVIDENCE_FILE")"
    MATRIX_SUMMARY_FILE="$(dirname "$EVIDENCE_FILE")/code-agent-matrix-summary.json"
  else
    matrix_temp="$(mktemp /tmp/cc-switch-server-matrix.XXXXXX.json)"
    MATRIX_SUMMARY_FILE="$matrix_temp"
  fi
fi
node scripts/code-agent-matrix-summary.mjs "$MATRIX_PATH" > "$MATRIX_SUMMARY_FILE"
MATRIX_TOTAL="$(read_matrix_field total)"
MATRIX_RUNNABLE="$(read_matrix_field runnable)"
MATRIX_SKIPPED="$(read_matrix_field skipped)"
MATRIX_SKELETON="$(read_matrix_field skeleton)"
echo "matrixPath=${MATRIX_PATH}"
echo "matrixSummary=${MATRIX_SUMMARY_FILE}"
echo "matrixTotal=${MATRIX_TOTAL} runnable=${MATRIX_RUNNABLE} skipped=${MATRIX_SKIPPED} skeleton=${MATRIX_SKELETON}"
node -e '
const fs = require("fs");
const data = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
for (const item of data.cases || []) {
  const status = item.runnable ? "runnable" : `skipped:${item.missing.join("|")}`;
  console.log(`- ${item.id} ${item.source} ${item.entryPath} ${status} adapter=${item.adapterStatus}`);
}
' "$MATRIX_SUMMARY_FILE"

echo "== contract tests =="
if [[ "$RUN_CONTRACT_TESTS" == "1" ]]; then
  cargo test proxy:: --quiet
  cargo test core::account_managers:: --quiet
  cargo test core::accounts:: --quiet
  cargo test core::oauth_clients:: --quiet
  pass "proxy/account contract tests"
else
  skip "contract tests disabled"
fi

echo "== server capability checks =="
if [[ -z "$API_TOKEN" ]]; then
  skip "CC_SWITCH_SERVER_TOKEN not set; skipped live server capability checks"
else
  for endpoint in /api/proxy/capabilities /api/accounts/capabilities /api/provider-coverage /api/usage/logs?limit=5; do
    status="$(curl -sS -o /tmp/cc-switch-server-regression-api.out -w "%{http_code}" "${auth_header[@]}" "$SERVER_URL$endpoint" || true)"
    echo "$endpoint status=${status}"
    if [[ "$status" =~ ^2 ]]; then
      pass "$endpoint"
    else
      fail "$endpoint"
    fi
  done
fi

echo "== local source probes =="
if [[ -n "$API_TOKEN" ]]; then
  if [[ -n "$CLAUDE_SHARE_ID" ]]; then
    probe "claude local messages non-stream" "$SERVER_URL/v1/messages" \
      '{"model":"probe","max_tokens":1,"messages":[{"role":"user","content":"ping"}],"stream":false}' \
      -H "X-CC-Switch-Share-Id: $CLAUDE_SHARE_ID" -H "X-CC-Switch-Data-Source: local"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "claude local messages stream" "$SERVER_URL/v1/messages" \
        '{"model":"probe","max_tokens":1,"messages":[{"role":"user","content":"stream ping"}],"stream":true}' \
        -H "X-CC-Switch-Share-Id: $CLAUDE_SHARE_ID" -H "X-CC-Switch-Data-Source: local"
    fi
  else
    skip "CLAUDE_SHARE_ID missing; skipped Claude local probes"
  fi

  if [[ -n "$CODEX_SHARE_ID" ]]; then
  probe "codex local responses non-stream" "$SERVER_URL/v1/responses" \
    '{"model":"probe","input":"ping","stream":false,"max_output_tokens":1}' \
    -H "X-CC-Switch-Share-Id: $CODEX_SHARE_ID" -H "X-CC-Switch-Data-Source: local"
  probe "codex local chat non-stream" "$SERVER_URL/v1/chat/completions" \
    '{"model":"probe","messages":[{"role":"user","content":"ping"}],"stream":false,"max_tokens":1}' \
    -H "X-CC-Switch-Share-Id: $CODEX_SHARE_ID" -H "X-CC-Switch-Data-Source: local"
  if [[ "$STREAM_PROBE" == "1" ]]; then
    stream_probe "codex local responses stream" "$SERVER_URL/v1/responses" \
      '{"model":"probe","input":"stream ping","stream":true,"max_output_tokens":1}' \
      -H "X-CC-Switch-Share-Id: $CODEX_SHARE_ID" -H "X-CC-Switch-Data-Source: local"
  fi
  else
    skip "CODEX_SHARE_ID/SHARE_ID missing; skipped Codex local probes"
  fi

  if [[ -n "$GEMINI_SHARE_ID" ]]; then
    probe "gemini local generateContent non-stream" "$SERVER_URL/v1beta/models/probe:generateContent" \
      '{"contents":[{"role":"user","parts":[{"text":"ping"}]}],"generationConfig":{"maxOutputTokens":1}}' \
      -H "X-CC-Switch-Share-Id: $GEMINI_SHARE_ID" -H "X-CC-Switch-Data-Source: local"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "gemini local generateContent stream" "$SERVER_URL/v1beta/models/probe:streamGenerateContent" \
        '{"contents":[{"role":"user","parts":[{"text":"stream ping"}]}],"generationConfig":{"maxOutputTokens":1}}' \
        -H "X-CC-Switch-Share-Id: $GEMINI_SHARE_ID" -H "X-CC-Switch-Data-Source: local"
    fi
  else
    skip "GEMINI_SHARE_ID missing; skipped Gemini local probes"
  fi
else
  skip "CC_SWITCH_SERVER_TOKEN missing; skipped local source probes"
fi

echo "== direct source probes =="
if [[ -n "$ROUTER_API_TOKEN" ]]; then
  if [[ -n "$DIRECT_CLAUDE_SHARE_URL" ]]; then
    probe "direct claude messages non-stream" "$DIRECT_CLAUDE_SHARE_URL/v1/messages" \
      '{"model":"probe","max_tokens":1,"messages":[{"role":"user","content":"ping"}],"stream":false}' \
      "${router_auth_header[@]}"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "direct claude messages stream" "$DIRECT_CLAUDE_SHARE_URL/v1/messages" \
        '{"model":"probe","max_tokens":1,"messages":[{"role":"user","content":"stream ping"}],"stream":true}' \
        "${router_auth_header[@]}"
    fi
  else
    skip "DIRECT_CLAUDE_SHARE_URL missing; skipped direct Claude probes"
  fi
  if [[ -n "$DIRECT_CODEX_SHARE_URL" ]]; then
    probe "direct codex responses non-stream" "$DIRECT_CODEX_SHARE_URL/v1/responses" \
    '{"model":"probe","input":"ping","stream":false,"max_output_tokens":1}' \
    "${router_auth_header[@]}"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "direct codex responses stream" "$DIRECT_CODEX_SHARE_URL/v1/responses" \
        '{"model":"probe","input":"stream ping","stream":true,"max_output_tokens":1}' \
        "${router_auth_header[@]}"
    fi
  else
    skip "DIRECT_CODEX_SHARE_URL/DIRECT_SHARE_URL missing; skipped direct Codex probes"
  fi
  if [[ -n "$DIRECT_GEMINI_SHARE_URL" ]]; then
    probe "direct gemini generateContent non-stream" "$DIRECT_GEMINI_SHARE_URL/v1beta/models/probe:generateContent" \
      '{"contents":[{"role":"user","parts":[{"text":"ping"}]}],"generationConfig":{"maxOutputTokens":1}}' \
      "${router_auth_header[@]}"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "direct gemini generateContent stream" "$DIRECT_GEMINI_SHARE_URL/v1beta/models/probe:streamGenerateContent" \
        '{"contents":[{"role":"user","parts":[{"text":"stream ping"}]}],"generationConfig":{"maxOutputTokens":1}}' \
        "${router_auth_header[@]}"
    fi
  else
    skip "DIRECT_GEMINI_SHARE_URL missing; skipped direct Gemini probes"
  fi
else
  skip "ROUTER_API_TOKEN missing; skipped direct source probes"
fi

echo "== market source probes =="
if [[ -n "$ROUTER_API_TOKEN" || -n "$MARKET_API_TOKEN" ]]; then
  if [[ -n "$MARKET_CLAUDE_API_URL" ]]; then
    probe "market claude messages non-stream" "$MARKET_CLAUDE_API_URL/v1/messages" \
      '{"model":"probe","max_tokens":1,"messages":[{"role":"user","content":"ping"}],"stream":false}' \
      "${market_auth_header[@]}"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "market claude messages stream" "$MARKET_CLAUDE_API_URL/v1/messages" \
        '{"model":"probe","max_tokens":1,"messages":[{"role":"user","content":"stream ping"}],"stream":true}' \
        "${market_auth_header[@]}"
    fi
  else
    skip "MARKET_CLAUDE_API_URL missing; skipped market Claude probes"
  fi
  if [[ -n "$MARKET_CODEX_API_URL" ]]; then
    probe "market codex responses non-stream" "$MARKET_CODEX_API_URL/v1/responses" \
    '{"model":"probe","input":"ping","stream":false,"max_output_tokens":1}' \
    "${market_auth_header[@]}"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "market codex responses stream" "$MARKET_CODEX_API_URL/v1/responses" \
        '{"model":"probe","input":"stream ping","stream":true,"max_output_tokens":1}' \
        "${market_auth_header[@]}"
    fi
  else
    skip "MARKET_CODEX_API_URL/MARKET_API_URL missing; skipped market Codex probes"
  fi
  if [[ -n "$MARKET_GEMINI_API_URL" ]]; then
    probe "market gemini generateContent non-stream" "$MARKET_GEMINI_API_URL/v1beta/models/probe:generateContent" \
      '{"contents":[{"role":"user","parts":[{"text":"ping"}]}],"generationConfig":{"maxOutputTokens":1}}' \
      "${market_auth_header[@]}"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "market gemini generateContent stream" "$MARKET_GEMINI_API_URL/v1beta/models/probe:streamGenerateContent" \
        '{"contents":[{"role":"user","parts":[{"text":"stream ping"}]}],"generationConfig":{"maxOutputTokens":1}}' \
        "${market_auth_header[@]}"
    fi
  else
    skip "MARKET_GEMINI_API_URL missing; skipped market Gemini probes"
  fi
else
  skip "ROUTER_API_TOKEN/MARKET_API_TOKEN missing; skipped market source probes"
fi

if [[ "$RUN_REAL" != "1" ]]; then
  echo "[INFO] RUN_REAL=0; real provider/OAuth success is not claimed."
fi

echo "== summary =="
echo "failures=${FAILURES} warnings=${WARNINGS} skipped=${SKIPPED}"
echo "matrixTotal=${MATRIX_TOTAL} matrixRunnable=${MATRIX_RUNNABLE} matrixSkipped=${MATRIX_SKIPPED} matrixSkeleton=${MATRIX_SKELETON}"

if [[ "$FAILURES" -gt 0 ]]; then
  BLOCKER_GROUP=""
  FAILURE_CLASS="provider-auth-or-transform"
elif [[ "$RUN_REAL" != "1" || "${MATRIX_SKIPPED:-0}" -gt 0 ]]; then
  BLOCKER_GROUP="missing-provider-token"
  FAILURE_CLASS=""
else
  BLOCKER_GROUP=""
  FAILURE_CLASS=""
fi

if [[ -n "$EVIDENCE_FILE" ]]; then
  if [[ "$FAILURES" -gt 0 ]]; then
    REGRESSION_EVIDENCE_STATUS="fail"
  elif [[ "$RUN_REAL" != "1" || "${MATRIX_SKIPPED:-0}" -gt 0 ]]; then
    REGRESSION_EVIDENCE_STATUS="ready-with-known-external-blockers"
  else
    REGRESSION_EVIDENCE_STATUS="pass"
  fi
  EVIDENCE_STAGE="${EVIDENCE_STAGE:-AB4-code-agent-regression}" \
  EVIDENCE_STATUS="$REGRESSION_EVIDENCE_STATUS" \
  FAILURES="$FAILURES" WARNINGS="$WARNINGS" \
  MATRIX_TOTAL="$MATRIX_TOTAL" MATRIX_RUNNABLE="$MATRIX_RUNNABLE" \
  MATRIX_SKIPPED="$MATRIX_SKIPPED" MATRIX_SKELETON="$MATRIX_SKELETON" \
  EVIDENCE_TARGET="code-agent-matrix" \
  BLOCKER_GROUP="$BLOCKER_GROUP" FAILURE_CLASS="$FAILURE_CLASS" \
  EVIDENCE_NOTES="skipped=${SKIPPED}; RUN_REAL=${RUN_REAL}; matrixSummary=${MATRIX_SUMMARY_FILE}" \
    node scripts/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
fi

if [[ -n "$matrix_temp" && -z "${KEEP_MATRIX_SUMMARY:-}" ]]; then
  rm -f "$matrix_temp"
fi

if [[ "$FAILURES" -gt 0 ]]; then
  exit 1
fi
