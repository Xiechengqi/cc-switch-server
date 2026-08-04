#!/usr/bin/env bash
set -euo pipefail

SERVER_URL="${SERVER_URL:-http://127.0.0.1:15721}"
API_TOKEN="${CC_SWITCH_SERVER_TOKEN:-}"
SHARE_URL="${CC_SWITCH_SHARE_URL:-}"
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
MATRIX_FIXTURE_EVIDENCE_COMPLETE=false
MATRIX_FIXTURE_EVIDENCE_MISSING=0
CONTRACT_TESTS_PASSED=0
CONTRACT_FAILURES=0

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

run_contract_test_group() {
  local label="$1"
  local filter="$2"
  local listing count
  if ! listing="$(cargo test --lib "$filter" -- --list)"; then
    fail "$label test listing"
    return
  fi
  count="$(printf '%s\n' "$listing" | awk '/: test$/ { count += 1 } END { print count + 0 }')"
  if [[ "$count" -le 0 ]]; then
    fail "$label matched zero tests"
    return
  fi
  if cargo test --lib "$filter" --quiet; then
    pass "$label (${count} tests)"
  else
    fail "$label"
  fi
}

run_contract_command() {
  local label="$1"
  shift
  if "$@"; then
    pass "$label"
  else
    fail "$label"
  fi
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
  if node scripts/smoke/stream-probe.mjs "${args[@]}"; then
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
node scripts/smoke/code-agent-matrix-summary.mjs "$MATRIX_PATH" > "$MATRIX_SUMMARY_FILE"
MATRIX_TOTAL="$(read_matrix_field total)"
MATRIX_RUNNABLE="$(read_matrix_field runnable)"
MATRIX_SKIPPED="$(read_matrix_field skipped)"
MATRIX_SKELETON="$(read_matrix_field skeleton)"
MATRIX_FIXTURE_EVIDENCE_COMPLETE="$(read_matrix_field fixtureEvidenceComplete)"
MATRIX_FIXTURE_EVIDENCE_MISSING="$(read_matrix_field fixtureEvidenceMissing)"
echo "matrixPath=${MATRIX_PATH}"
echo "matrixSummary=${MATRIX_SUMMARY_FILE}"
echo "matrixTotal=${MATRIX_TOTAL} runnable=${MATRIX_RUNNABLE} skipped=${MATRIX_SKIPPED} skeleton=${MATRIX_SKELETON} fixtureEvidenceComplete=${MATRIX_FIXTURE_EVIDENCE_COMPLETE} fixtureEvidenceMissing=${MATRIX_FIXTURE_EVIDENCE_MISSING}"
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
  contract_failures_before="$FAILURES"
  run_contract_test_group "proxy contract tests" "proxy::"
  run_contract_test_group "account domain contract tests" "domain::accounts::"
  run_contract_test_group "OAuth client contract tests" "clients::oauth::"
  run_contract_command "provider coverage audit" node scripts/audit/audit-provider-coverage.mjs --check
  run_contract_command "UI provider matrix audit" node scripts/audit/audit-ui-provider-matrix.mjs --check
  run_contract_command "proxy bridge contract audit" node scripts/audit/audit-proxy-bridge-contract.mjs --check
  run_contract_command "Web runtime contract audit" node scripts/audit/audit-web-runtime-contract.mjs --check
  if [[ -d web-src/node_modules ]]; then
    run_contract_command "Web typecheck" npm --prefix web-src run typecheck
    run_contract_command "Web unit tests" npm --prefix web-src run test
  else
    fail "Web contract tests require web-src/node_modules"
  fi
  if [[ "$FAILURES" -eq "$contract_failures_before" ]]; then
    CONTRACT_TESTS_PASSED=1
  fi
  CONTRACT_FAILURES=$((FAILURES - contract_failures_before))
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

echo "== router share probes =="
if [[ -n "$SHARE_URL" && -n "$ROUTER_API_TOKEN" ]]; then
    probe "share claude messages non-stream" "$SHARE_URL/v1/messages" \
      '{"model":"probe","max_tokens":1,"messages":[{"role":"user","content":"ping"}],"stream":false}' \
      "${router_auth_header[@]}"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "share claude messages stream" "$SHARE_URL/v1/messages" \
        '{"model":"probe","max_tokens":1,"messages":[{"role":"user","content":"stream ping"}],"stream":true}' \
        "${router_auth_header[@]}"
    fi
    probe "share codex responses non-stream" "$SHARE_URL/v1/responses" \
    '{"model":"probe","input":"ping","stream":false,"max_output_tokens":1}' \
    "${router_auth_header[@]}"
    probe "share codex chat non-stream" "$SHARE_URL/v1/chat/completions" \
      '{"model":"probe","messages":[{"role":"user","content":"ping"}],"stream":false,"max_tokens":1}' \
      "${router_auth_header[@]}"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "share codex responses stream" "$SHARE_URL/v1/responses" \
        '{"model":"probe","input":"stream ping","stream":true,"max_output_tokens":1}' \
        "${router_auth_header[@]}"
      stream_probe "share codex chat stream" "$SHARE_URL/v1/chat/completions" \
        '{"model":"probe","messages":[{"role":"user","content":"stream ping"}],"stream":true,"max_tokens":1}' \
        "${router_auth_header[@]}"
    fi
    probe "share gemini generateContent non-stream" "$SHARE_URL/v1beta/models/probe:generateContent" \
      '{"contents":[{"role":"user","parts":[{"text":"ping"}]}],"generationConfig":{"maxOutputTokens":1}}' \
      "${router_auth_header[@]}"
    if [[ "$STREAM_PROBE" == "1" ]]; then
      stream_probe "share gemini generateContent stream" "$SHARE_URL/v1beta/models/probe:streamGenerateContent" \
        '{"contents":[{"role":"user","parts":[{"text":"stream ping"}]}],"generationConfig":{"maxOutputTokens":1}}' \
        "${router_auth_header[@]}"
    fi
else
  skip "CC_SWITCH_SHARE_URL or ROUTER_API_TOKEN missing; skipped Router Share probes"
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
echo "matrixTotal=${MATRIX_TOTAL} matrixRunnable=${MATRIX_RUNNABLE} matrixSkipped=${MATRIX_SKIPPED} matrixSkeleton=${MATRIX_SKELETON} fixtureEvidenceComplete=${MATRIX_FIXTURE_EVIDENCE_COMPLETE} fixtureEvidenceMissing=${MATRIX_FIXTURE_EVIDENCE_MISSING}"

gate_temp="$(mktemp /tmp/cc-switch-server-evidence-gate.XXXXXX.json)"
FAILURES="$FAILURES" CONTRACT_FAILURES="$CONTRACT_FAILURES" SKIPPED="$SKIPPED" \
MATRIX_TOTAL="$MATRIX_TOTAL" MATRIX_RUNNABLE="$MATRIX_RUNNABLE" \
MATRIX_SKIPPED="$MATRIX_SKIPPED" RUN_REAL="$RUN_REAL" \
RUN_CONTRACT_TESTS="$RUN_CONTRACT_TESTS" \
CONTRACT_TESTS_PASSED="$CONTRACT_TESTS_PASSED" \
STREAM_PROBE="$STREAM_PROBE" REQUIRE_STREAM_USAGE="$REQUIRE_STREAM_USAGE" \
MATRIX_FIXTURE_EVIDENCE_COMPLETE="$MATRIX_FIXTURE_EVIDENCE_COMPLETE" \
  node scripts/smoke/code-agent-evidence-gate.mjs > "$gate_temp"

read_gate_field() {
  local field="$1"
  node -e '
const fs = require("fs");
const data = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const value = data[process.argv[2]];
process.stdout.write(Array.isArray(value) ? value.join(",") : String(value ?? ""));
' "$gate_temp" "$field"
}

if [[ "$(read_gate_field liveVerificationComplete)" == "true" ]]; then
  LIVE_VERIFICATION_COMPLETE=1
else
  LIVE_VERIFICATION_COMPLETE=0
fi
REGRESSION_EVIDENCE_STATUS="$(read_gate_field status)"
REGRESSION_VERIFICATION_STATE="$(read_gate_field verificationState)"
BLOCKER_GROUP="$(read_gate_field blockerGroup)"
BLOCKED_GROUPS="$(read_gate_field blockerGroups)"
FAILURE_CLASS="$(read_gate_field failureClass)"
echo "verificationState=${REGRESSION_VERIFICATION_STATE} blockerGroup=${BLOCKER_GROUP:-none} blockedGroups=${BLOCKED_GROUPS:-none}"

if [[ -n "$EVIDENCE_FILE" ]]; then
  EVIDENCE_STAGE="${EVIDENCE_STAGE:-AB4-code-agent-regression}" \
  EVIDENCE_STATUS="$REGRESSION_EVIDENCE_STATUS" \
  EVIDENCE_VERIFICATION_STATE="$REGRESSION_VERIFICATION_STATE" \
  EVIDENCE_VERIFICATION_SCOPE="configured_matrix_routes" \
  RUN_REAL="$RUN_REAL" \
  RUN_CONTRACT_TESTS="$RUN_CONTRACT_TESTS" \
  CONTRACT_TESTS_PASSED="$CONTRACT_TESTS_PASSED" \
  CONTRACT_FAILURES="$CONTRACT_FAILURES" \
  STREAM_PROBE="$STREAM_PROBE" REQUIRE_STREAM_USAGE="$REQUIRE_STREAM_USAGE" \
  LIVE_VERIFICATION_COMPLETE="$LIVE_VERIFICATION_COMPLETE" \
  FAILURES="$FAILURES" WARNINGS="$WARNINGS" SKIPPED="$SKIPPED" \
  MATRIX_TOTAL="$MATRIX_TOTAL" MATRIX_RUNNABLE="$MATRIX_RUNNABLE" \
  MATRIX_SKIPPED="$MATRIX_SKIPPED" MATRIX_SKELETON="$MATRIX_SKELETON" \
  MATRIX_FIXTURE_EVIDENCE_COMPLETE="$MATRIX_FIXTURE_EVIDENCE_COMPLETE" \
  MATRIX_FIXTURE_EVIDENCE_MISSING="$MATRIX_FIXTURE_EVIDENCE_MISSING" \
  EVIDENCE_TARGET="code-agent-matrix" \
  BLOCKER_GROUP="$BLOCKER_GROUP" BLOCKED_GROUPS="$BLOCKED_GROUPS" \
  FAILURE_CLASS="$FAILURE_CLASS" \
  EVIDENCE_NOTES="skipped=${SKIPPED}; RUN_REAL=${RUN_REAL}; streamProbe=${STREAM_PROBE}; requireStreamUsage=${REQUIRE_STREAM_USAGE}; matrixSummary=${MATRIX_SUMMARY_FILE}" \
    node scripts/smoke/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
fi

if [[ -n "$matrix_temp" && -z "${KEEP_MATRIX_SUMMARY:-}" ]]; then
  rm -f "$matrix_temp"
fi
rm -f "$gate_temp"

if [[ "$FAILURES" -gt 0 ]]; then
  exit 1
fi
