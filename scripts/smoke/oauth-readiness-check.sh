#!/usr/bin/env bash
set -euo pipefail

SERVER_URL="${SERVER_URL:-http://127.0.0.1:15721}"
API_TOKEN="${CC_SWITCH_SERVER_TOKEN:-}"
EVIDENCE_FILE="${EVIDENCE_FILE:-}"
RUN_LOCAL_TESTS="${RUN_LOCAL_TESTS:-1}"
FAILURES=0
WARNINGS=0
SKELETON_TOTAL=0
OAUTH_NATIVE_READY=false
OAUTH_GATE_STATUS="unknown"

pass() { echo "[PASS] $*"; }
warn() { WARNINGS=$((WARNINGS + 1)); echo "[WARN] $*"; }
fail() { FAILURES=$((FAILURES + 1)); echo "[FAIL] $*"; }

env_present() {
  local name="$1"
  [[ -n "${!name:-}" && "${!name}" != \<* ]]
}

join_missing() {
  local output=""
  local item
  for item in "$@"; do
    if [[ -z "$output" ]]; then
      output="$item"
    else
      output="${output}, ${item}"
    fi
  done
  printf '%s' "$output"
}

check_required_vars() {
  local label="$1"
  shift
  local missing=()
  local var
  for var in "$@"; do
    if ! env_present "$var"; then
      missing+=("$var")
    fi
  done
  if [[ "${#missing[@]}" -eq 0 ]]; then
    pass "$label inputs present"
  else
    warn "$label blocked by missing $(join_missing "${missing[@]}")"
  fi
}

check_any_var() {
  local label="$1"
  shift
  local var
  for var in "$@"; do
    if env_present "$var"; then
      pass "$label input present via $var"
      return
    fi
  done
  warn "$label blocked; set at least one of $(join_missing "$@")"
}

echo "== upstream source presence =="
sources=(
  "/data/projects/cc-switch/src-tauri/src/proxy/providers/codex_oauth_auth.rs"
  "/data/projects/cc-switch/src-tauri/src/proxy/providers/codex.rs"
  "/data/projects/cc-switch/src-tauri/src/proxy/codex_responses_ws.rs"
  "/data/projects/cc-switch/src-tauri/src/proxy/providers/claude_oauth_auth.rs"
  "/data/projects/cc-switch/src-tauri/src/proxy/providers/gemini_oauth_auth.rs"
  "/data/projects/cc-switch/src-tauri/src/proxy/providers/gemini.rs"
  "/data/projects/cc-switch/src-tauri/src/proxy/providers/cursor_oauth_auth.rs"
  "/data/projects/cc-switch/src-tauri/src/proxy/providers/copilot_auth.rs"
  "/data/projects/cc-switch/src-tauri/src/proxy/providers/kiro_oauth_auth.rs"
  "/data/projects/cc-switch/src-tauri/src/proxy/providers/antigravity_oauth_auth.rs"
)
for source in "${sources[@]}"; do
  if [[ -f "$source" ]]; then
    pass "source exists: ${source#/data/projects/cc-switch/}"
  else
    warn "source missing: ${source#/data/projects/cc-switch/}"
  fi
done

echo "== local fixtures =="
if [[ "$RUN_LOCAL_TESTS" == "1" ]]; then
  cargo test core::account_managers:: --quiet
  cargo test core::accounts:: --quiet
  cargo test core::oauth_clients:: --quiet
  cargo test proxy::adapters:: --quiet
  pass "account, oauth client, and adapter fixture tests"
else
  warn "RUN_LOCAL_TESTS=0; skipped cargo fixture tests"
fi

echo "== skeleton adapter visibility =="
SKELETON_TOTAL="$( (rg -o '"[^"]*_skeleton"' src/proxy/adapters.rs || true) | wc -l | tr -d ' ')"
echo "skeletonAdapters=${SKELETON_TOTAL}"
(rg -o '"[^"]*_skeleton"' src/proxy/adapters.rs || true) | sort -u | sed 's/^/- /'
if [[ "$SKELETON_TOTAL" -gt 0 ]]; then
  pass "skeleton adapters remain visible before real adapter evidence"
else
  warn "no skeleton adapters found; verify production checklist before claiming long-tail coverage"
fi

echo "== capability guard =="
if [[ -z "$API_TOKEN" ]]; then
  warn "CC_SWITCH_SERVER_TOKEN not set; skipped live capability guard"
  OAUTH_GATE_STATUS="blocked-inputs"
else
  accounts="$(curl -fsS -H "Authorization: Bearer $API_TOKEN" "$SERVER_URL/api/accounts/capabilities")"
  proxy="$(curl -fsS -H "Authorization: Bearer $API_TOKEN" "$SERVER_URL/api/proxy/capabilities")"
  if printf '%s' "$accounts" | jq -e '.capabilities[] | select(.support == "native_oauth" or .supportsStartLogin == true)' >/dev/null; then
    fail "account capability prematurely exposes native OAuth browser login"
  else
    pass "account capabilities remain manual-login before real OAuth browser validation"
  fi
  if printf '%s' "$proxy" | jq -e '.capabilities[] | select(.supportsOauthRefresh == true)' >/dev/null; then
    fail "proxy capability prematurely exposes OAuth refresh"
  else
    pass "proxy capabilities do not claim OAuth refresh yet"
  fi
  OAUTH_GATE_STATUS="guarded"
fi

echo "== real credential gates =="
check_required_vars "AB5 Codex OAuth" CODEX_OAUTH_TEST_ACCOUNT CODEX_OAUTH_CALLBACK_URL
check_any_var "AB5 Codex refresh/import fixture" CODEX_OAUTH_REFRESH_TOKEN_FIXTURE CODEX_OAUTH_REFRESH_TOKEN
check_required_vars "AB6 Claude OAuth" CLAUDE_OAUTH_TEST_ACCOUNT CLAUDE_OAUTH_CALLBACK_URL
check_any_var "AB6 Claude refresh/import fixture" CLAUDE_OAUTH_REFRESH_TOKEN_FIXTURE CLAUDE_OAUTH_REFRESH_TOKEN
check_required_vars "AB6 Gemini OAuth" GEMINI_OAUTH_TEST_ACCOUNT GEMINI_OAUTH_CALLBACK_URL
check_any_var "AB6 Gemini refresh/import fixture" GEMINI_OAUTH_REFRESH_TOKEN_FIXTURE GEMINI_OAUTH_REFRESH_TOKEN GEMINI_CLI_CREDENTIALS_FIXTURE
check_required_vars "AB6 Antigravity/Agy OAuth" ANTIGRAVITY_OAUTH_TEST_ACCOUNT ANTIGRAVITY_OAUTH_CALLBACK_URL
check_any_var "AB6 Antigravity/Agy refresh/import fixture" ANTIGRAVITY_OAUTH_REFRESH_TOKEN_FIXTURE
check_required_vars "AB7 Cursor OAuth" CURSOR_OAUTH_TEST_ACCOUNT CURSOR_OAUTH_CALLBACK_URL
check_any_var "AB7 Cursor credential fixture" CURSOR_OAUTH_REFRESH_TOKEN_FIXTURE CURSOR_API_KEY_FIXTURE
check_required_vars "AB7 GitHub Copilot device flow" GITHUB_COPILOT_TEST_ACCOUNT
check_required_vars "AB7 Kiro device flow" KIRO_TEST_ACCOUNT KIRO_REGION KIRO_START_URL
check_any_var "AB7 Kiro refresh/import fixture" KIRO_REFRESH_TOKEN_FIXTURE
check_required_vars "AB7 AWS Bedrock signed request" AWS_REGION AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY BEDROCK_MODEL_ID

echo "== summary =="
if [[ "$FAILURES" -gt 0 ]]; then
  OAUTH_GATE_STATUS="fail"
fi
echo "failures=${FAILURES} warnings=${WARNINGS}"
echo "oauthNativeReady=${OAUTH_NATIVE_READY} oauthGateStatus=${OAUTH_GATE_STATUS} skeletonTotal=${SKELETON_TOTAL}"
if [[ -n "$EVIDENCE_FILE" ]]; then
  EVIDENCE_STAGE="${EVIDENCE_STAGE:-AB5-AB7-oauth-readiness}" \
  EVIDENCE_STATUS="$([[ "$FAILURES" -eq 0 ]] && echo pass || echo fail)" \
  OAUTH_NATIVE_READY="$OAUTH_NATIVE_READY" OAUTH_GATE_STATUS="$OAUTH_GATE_STATUS" \
  CURSOR_GATE_STATUS="$([[ "$WARNINGS" -eq 0 ]] && echo ready || echo blocked-inputs)" \
  COPILOT_GATE_STATUS="$([[ "$WARNINGS" -eq 0 ]] && echo ready || echo blocked-inputs)" \
  KIRO_GATE_STATUS="$([[ "$WARNINGS" -eq 0 ]] && echo ready || echo blocked-inputs)" \
  BEDROCK_GATE_STATUS="$([[ "$WARNINGS" -eq 0 ]] && echo ready || echo blocked-inputs)" \
  SKELETON_TOTAL="$SKELETON_TOTAL" \
  FAILURES="$FAILURES" WARNINGS="$WARNINGS" \
    node scripts/smoke/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
fi

if [[ "$FAILURES" -gt 0 ]]; then
  exit 1
fi
