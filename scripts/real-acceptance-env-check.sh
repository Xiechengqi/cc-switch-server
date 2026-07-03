#!/usr/bin/env bash
set -euo pipefail

STRICT="${STRICT:-0}"
STAGE="${STAGE:-all}"
EVIDENCE_FILE="${EVIDENCE_FILE:-}"
FAILED_GROUPS=0

normalize_stage() {
  local value
  value="$(printf '%s' "$1" | tr '[:lower:]' '[:upper:]')"
  case "$value" in
    AC1) value="AB1" ;;
    AC2) value="AB2" ;;
    AC3) value="AB3" ;;
    AC4) value="AB4" ;;
    AC5) value="AB5" ;;
    AC6) value="AB6" ;;
    AC7) value="AB7" ;;
    AC8) value="AB8" ;;
    AA2) value="AB2" ;;
    AA3) value="AB3" ;;
    AA4) value="AB4" ;;
    AA5) value="AB5" ;;
    AA6) value="AB6" ;;
    AA7) value="AB7" ;;
    AA8) value="AB8" ;;
    ALL|"") value="ALL" ;;
  esac
  printf '%s' "$value"
}

STAGE="$(normalize_stage "$STAGE")"

should_run() {
  local stage="$1"
  [[ "$STAGE" == "ALL" || "$STAGE" == "$stage" ]]
}

is_set() {
  local name="$1"
  local value="${!name:-}"
  [[ -n "$value" && "$value" != \<* ]]
}

join_missing() {
  local missing=("$@")
  local output=""
  local item
  for item in "${missing[@]}"; do
    if [[ -z "$output" ]]; then
      output="$item"
    else
      output="${output}, ${item}"
    fi
  done
  printf '%s' "$output"
}

check_group() {
  local label="$1"
  shift
  local missing=()
  local name
  for name in "$@"; do
    if ! is_set "$name"; then
      missing+=("$name")
    fi
  done

  if [[ "${#missing[@]}" -eq 0 ]]; then
    echo "[READY] ${label}"
  else
    FAILED_GROUPS=$((FAILED_GROUPS + 1))
    echo "[BLOCKED] ${label}: missing $(join_missing "${missing[@]}")"
  fi
}

check_any() {
  local label="$1"
  shift
  local name
  for name in "$@"; do
    if is_set "$name"; then
      echo "[READY] ${label}: using ${name}"
      return
    fi
  done
  FAILED_GROUPS=$((FAILED_GROUPS + 1))
  echo "[BLOCKED] ${label}: set at least one of $(join_missing "$@")"
}

check_optional() {
  local name="$1"
  if is_set "$name"; then
    echo "[SET] ${name}"
  else
    echo "[OPTIONAL] ${name} is not set"
  fi
}

check_header() {
  local name="$1"
  local value="${!name:-}"
  if [[ -z "$value" ]]; then
    return 0
  fi
  case "$value" in
    Authorization|authorization|x-api-key|X-API-Key|x-goog-api-key|X-Goog-Api-Key)
      echo "[OK] ${name} is supported"
      ;;
    *)
      FAILED_GROUPS=$((FAILED_GROUPS + 1))
      echo "[BLOCKED] ${name}: unsupported header name"
      ;;
  esac
}

check_stream_probe() {
  case "${STREAM_PROBE:-0}" in
    0|1)
      echo "[OK] STREAM_PROBE=${STREAM_PROBE:-0}"
      ;;
    *)
      FAILED_GROUPS=$((FAILED_GROUPS + 1))
      echo "[BLOCKED] STREAM_PROBE must be 0 or 1"
      ;;
  esac
  case "${REQUIRE_STREAM_USAGE:-0}" in
    0|1)
      echo "[OK] REQUIRE_STREAM_USAGE=${REQUIRE_STREAM_USAGE:-0}"
      ;;
    *)
      FAILED_GROUPS=$((FAILED_GROUPS + 1))
      echo "[BLOCKED] REQUIRE_STREAM_USAGE must be 0 or 1"
      ;;
  esac
}

echo "== cc-switch-server real acceptance env check =="
echo "stage=${STAGE}"
echo "No secret values are printed."

if should_run "AB1"; then
  echo "== AB1 local bootstrap =="
  echo "[READY] AB1 static checks can run without external secrets: scripts/static-checks.sh"
  echo "[READY] AB1 full local smoke can run when compile/service start is allowed: scripts/smoke-local.sh"
fi

if should_run "AB2" || should_run "AB3" || should_run "AB4" || should_run "AB8"; then
  echo "== baseline =="
  check_group "server auth" SERVER_URL CC_SWITCH_SERVER_TOKEN
  check_optional ROUTER_BASE_URL
  check_optional MARKET_URL
  check_header ROUTER_API_TOKEN_HEADER
  check_header MARKET_API_TOKEN_HEADER
  check_header SHARE_MARKET_GRANT_TOKEN_HEADER
  check_stream_probe
fi

if should_run "AB2"; then
  echo "== AB2 direct public share URL =="
  check_group "AB2 authenticated direct public probe" SERVER_URL CC_SWITCH_SERVER_TOKEN SHARE_ID DIRECT_SHARE_URL ROUTER_API_TOKEN
fi

if should_run "AB3"; then
  echo "== AB3 market API URL =="
  check_group "AB3 market dispatch base" SERVER_URL CC_SWITCH_SERVER_TOKEN MARKET_API_URL
  check_any "AB3 market auth token" ROUTER_API_TOKEN MARKET_API_TOKEN
fi

if should_run "AB4"; then
  echo "== AB4 code agent regression =="
  check_group "AB4 local regression" SERVER_URL CC_SWITCH_SERVER_TOKEN SHARE_ID
  check_group "AB4 real provider tokens" CLAUDE_PROVIDER_TOKEN CODEX_PROVIDER_TOKEN GEMINI_PROVIDER_TOKEN
fi

if should_run "AB5"; then
  echo "== AB5 Codex OAuth =="
  check_group "AB5 Codex OAuth real account" CODEX_OAUTH_TEST_ACCOUNT CODEX_OAUTH_CALLBACK_URL
  check_optional CODEX_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional CODEX_OAUTH_REFRESH_TOKEN
fi

if should_run "AB6"; then
  echo "== AB6 Claude/Gemini/Antigravity OAuth =="
  check_group "AB6 Claude OAuth real account" CLAUDE_OAUTH_TEST_ACCOUNT CLAUDE_OAUTH_CALLBACK_URL
  check_optional CLAUDE_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional CLAUDE_OAUTH_REFRESH_TOKEN
  check_group "AB6 Gemini OAuth real account" GEMINI_OAUTH_TEST_ACCOUNT GEMINI_OAUTH_CALLBACK_URL
  check_optional GEMINI_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional GEMINI_OAUTH_REFRESH_TOKEN
  check_optional GEMINI_CLI_CREDENTIALS_FIXTURE
  check_group "AB6 Antigravity/Agy OAuth real account" ANTIGRAVITY_OAUTH_TEST_ACCOUNT ANTIGRAVITY_OAUTH_CALLBACK_URL
  check_optional ANTIGRAVITY_OAUTH_REFRESH_TOKEN_FIXTURE
fi

if should_run "AB7"; then
  echo "== AB7 long-tail providers =="
  check_group "AB7 Cursor OAuth real account" CURSOR_OAUTH_TEST_ACCOUNT CURSOR_OAUTH_CALLBACK_URL
  check_any "AB7 Cursor credential fixture" CURSOR_OAUTH_REFRESH_TOKEN_FIXTURE CURSOR_API_KEY_FIXTURE
  check_group "AB7 GitHub Copilot device flow account" GITHUB_COPILOT_TEST_ACCOUNT
  check_optional GITHUB_COPILOT_GITHUB_DOMAIN
  check_optional GITHUB_COPILOT_TOKEN_FIXTURE
  check_group "AB7 Kiro device flow account" KIRO_TEST_ACCOUNT KIRO_REGION KIRO_START_URL
  check_optional KIRO_REFRESH_TOKEN_FIXTURE
  check_group "AB7 AWS Bedrock signed request credentials" AWS_REGION AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY BEDROCK_MODEL_ID
  check_optional AWS_SESSION_TOKEN
  echo "[INFO] These inputs only unblock real validation; they do not enable NativeOAuth/native adapter capability by themselves."
fi

if should_run "AB8"; then
  echo "== AB8 share-market grant =="
  check_group "AB8 share-market grant add/revoke" SHARE_MARKET_URL SHARE_MARKET_GRANT_TOKEN SHARE_MARKET_BUYER_EMAIL SHARE_MARKET_LISTING_ID SHARE_MARKET_ORDER_ID
  check_optional SHARE_MARKET_APP_TYPE
  check_optional SHARE_MARKET_GRANT_POLL_SECONDS
fi

echo "== summary =="
echo "blocked_groups=${FAILED_GROUPS}"

if [[ -n "$EVIDENCE_FILE" ]]; then
  BLOCKED_GROUPS="$FAILED_GROUPS" \
  EVIDENCE_STAGE="${EVIDENCE_STAGE:-${STAGE}-env-check}" \
  EVIDENCE_STATUS="$([[ "$FAILED_GROUPS" -eq 0 ]] && echo ready || echo blocked)" \
    node scripts/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
fi

if [[ "$STRICT" == "1" && "$FAILED_GROUPS" -gt 0 ]]; then
  exit 2
fi
