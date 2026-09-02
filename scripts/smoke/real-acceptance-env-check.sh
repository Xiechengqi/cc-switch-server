#!/usr/bin/env bash
set -euo pipefail

STRICT="${STRICT:-0}"
STAGE="${STAGE:-all}"
EVIDENCE_FILE="${EVIDENCE_FILE:-}"
FAILED_GROUPS=0
EXTERNAL_BLOCKED_GROUPS=0

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

gate_status() {
  local name
  for name in "$@"; do
    if ! is_set "$name"; then
      printf '%s' "blocked-inputs"
      return
    fi
  done
  printf '%s' "inputs-ready"
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

check_external_group() {
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
    echo "[EXTERNAL-READY] ${label}: inputs present; real validation has not run"
  else
    EXTERNAL_BLOCKED_GROUPS=$((EXTERNAL_BLOCKED_GROUPS + 1))
    echo "[EXTERNAL-BLOCKED] ${label}: missing $(join_missing "${missing[@]}")"
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

check_binary_flag() {
  local name="$1"
  local value="${!name:-0}"
  case "$value" in
    0|1)
      echo "[OK] ${name}=${value}"
      ;;
    *)
      FAILED_GROUPS=$((FAILED_GROUPS + 1))
      echo "[BLOCKED] ${name} must be 0 or 1"
      ;;
  esac
}

echo "== cc-switch-server real acceptance env check =="
echo "stage=${STAGE}"
echo "No secret values are printed."

if should_run "AB1"; then
  echo "== AB1 local bootstrap =="
  echo "[READY] AB1 static checks can run without external secrets: scripts/static-checks.sh"
  echo "[READY] AB1 full local smoke can run when compile/service start is allowed: scripts/smoke/smoke-local.sh"
fi

if should_run "AB2" || should_run "AB3" || should_run "AB4" || should_run "AB8"; then
  echo "== baseline =="
  check_group "server auth" SERVER_URL CC_SWITCH_SERVER_TOKEN
  check_optional ROUTER_BASE_URL
  check_header ROUTER_API_TOKEN_HEADER
  check_stream_probe
fi

if should_run "AB2"; then
  echo "== AB2 Router Share URL =="
  check_group "AB2 authenticated Router Share probe" SERVER_URL CC_SWITCH_SERVER_TOKEN SHARE_ID CC_SWITCH_SHARE_URL ROUTER_API_TOKEN
fi

if should_run "AB3"; then
  echo "== AB3 Client + Router Gateway/Share =="
  check_group "AB3 Router Gateway/Share base" SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN
fi

if should_run "AB4"; then
  echo "== AB4 code agent regression =="
  check_group "AB4 Router Share regression" SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN
  check_group "AB4 real provider tokens" CLAUDE_PROVIDER_TOKEN CODEX_PROVIDER_TOKEN GEMINI_PROVIDER_TOKEN
  check_group "AB4 complete fixture evidence" MATRIX_LIVE_EVIDENCE_FILE
fi

if should_run "AB5"; then
  echo "== AB5 Codex OAuth =="
  check_group "AB5 Codex OAuth real account" CODEX_OAUTH_TEST_ACCOUNT CODEX_OAUTH_CALLBACK_URL
  check_optional CODEX_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional CODEX_OAUTH_REFRESH_TOKEN
  check_binary_flag CC_SWITCH_CODEX_IMAGES_SMOKE
  if [[ "${CC_SWITCH_CODEX_IMAGES_SMOKE:-0}" == "1" ]]; then
    check_external_group "AB5 Codex Images Router Share smoke" CC_SWITCH_SHARE_URL ROUTER_API_TOKEN
    echo "[INFO] Run node scripts/smoke/codex-images-real.mjs; input readiness is not live acceptance."
  else
    echo "[OPTIONAL] Codex Images Cloudflare smoke is disabled"
  fi
fi

if should_run "AB6"; then
  echo "== AB6 Claude/Gemini/Antigravity OAuth =="
  check_group "AB6 Claude OAuth real account" CLAUDE_OAUTH_TEST_ACCOUNT CLAUDE_OAUTH_CALLBACK_URL
  check_optional CLAUDE_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional CLAUDE_OAUTH_REFRESH_TOKEN
  echo "== AB6 Claude Max multiplier external gates =="
  check_external_group "AB6 Claude Max 5x plan resolution" CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT
  check_external_group "AB6 Claude Max 20x plan resolution" CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT
  echo "[INFO] Each Max multiplier requires its own real OAuth account before live acceptance can be claimed."
  check_group "AB6 Gemini OAuth real account" GEMINI_OAUTH_TEST_ACCOUNT GEMINI_OAUTH_CALLBACK_URL
  check_optional GEMINI_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional GEMINI_OAUTH_REFRESH_TOKEN
  check_optional GEMINI_CLI_CREDENTIALS_FIXTURE
  check_group "AB6 Antigravity/Agy OAuth real account" ANTIGRAVITY_OAUTH_TEST_ACCOUNT ANTIGRAVITY_OAUTH_CALLBACK_URL
  check_optional ANTIGRAVITY_OAUTH_REFRESH_TOKEN_FIXTURE
  echo "== AB6 Grok OAuth external gate =="
  check_external_group "AB6 Grok OAuth single-account smoke" GROK_OAUTH_TEST_ACCOUNT CC_SWITCH_SHARE_URL ROUTER_API_TOKEN
  check_optional GROK_OAUTH_CALLBACK_URL
  check_optional GROK_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional GROK_OAUTH_AUTH_JSON_FIXTURE
  check_optional CC_SWITCH_GROK_MODEL
  check_optional CC_SWITCH_GROK_MEDIA_SMOKE
  echo "[INFO] Grok input readiness is external evidence only; run scripts/smoke/grok-oauth-real.mjs before claiming live acceptance."
fi

if should_run "AB7"; then
  echo "== AB7 long-tail providers =="
  check_group "AB7 Cursor OAuth real account" CURSOR_OAUTH_TEST_ACCOUNT CURSOR_OAUTH_CALLBACK_URL
  check_any "AB7 Cursor credential fixture" CURSOR_OAUTH_REFRESH_TOKEN_FIXTURE CURSOR_API_KEY_FIXTURE
  check_external_group "AB7 Qoder Global OAuth bound-account three-surface smoke" QODER_GLOBAL_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_QODER_GLOBAL_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_OAUTH_CODEX_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_OAUTH_GEMINI_PROVIDER_ID
  check_external_group "AB7 Qoder Global PAT bound-account three-surface smoke" QODER_GLOBAL_PAT_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_QODER_GLOBAL_PAT_CLAUDE_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_PAT_CODEX_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_PAT_GEMINI_PROVIDER_ID
  check_external_group "AB7 Qoder CN OAuth bound-account three-surface smoke" QODER_CN_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_QODER_CN_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_QODER_CN_OAUTH_CODEX_PROVIDER_ID CC_SWITCH_QODER_CN_OAUTH_GEMINI_PROVIDER_ID
  check_optional CC_SWITCH_QODER_GLOBAL_OAUTH_MODEL
  check_optional CC_SWITCH_QODER_GLOBAL_PAT_MODEL
  check_optional CC_SWITCH_QODER_CN_OAUTH_MODEL
  check_optional QODER_REAL_RECEIPT_FILE
  echo "[INFO] Run scripts/smoke/qoder-real.mjs once per rail; input readiness is not live acceptance."
  check_external_group "AB7 GitHub Copilot bound-account three-surface smoke" GITHUB_COPILOT_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID CC_SWITCH_COPILOT_CODEX_PROVIDER_ID CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID
  check_optional GITHUB_COPILOT_GITHUB_DOMAIN
  check_optional GITHUB_COPILOT_TOKEN_FIXTURE
  check_optional CC_SWITCH_COPILOT_MODEL
  echo "[INFO] Run node scripts/smoke/copilot-real.mjs; input readiness is not live acceptance."
  check_group "AB7 Kiro device flow account" KIRO_TEST_ACCOUNT KIRO_REGION KIRO_START_URL
  check_optional KIRO_REFRESH_TOKEN_FIXTURE
  check_external_group "AB7 Amazon Q bound-account two-surface smoke" AMAZON_Q_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_AMAZON_Q_CLAUDE_PROVIDER_ID CC_SWITCH_AMAZON_Q_CODEX_PROVIDER_ID
  check_optional AMAZON_Q_REFRESH_TOKEN_FIXTURE
  check_optional CC_SWITCH_AMAZON_Q_MODEL
  check_optional CC_SWITCH_AMAZON_Q_RUNTIME_REGION
  check_optional CC_SWITCH_AMAZON_Q_PROFILE_ARN
  echo "[INFO] Amazon Q is independent from Kiro; input readiness is not live acceptance."
  check_group "AB7 AWS Bedrock signed request credentials" AWS_REGION AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY BEDROCK_MODEL_ID
  check_optional AWS_SESSION_TOKEN
  echo "[INFO] These inputs only unblock real validation; they do not enable NativeOAuth/native adapter capability by themselves."
fi

if should_run "AB8"; then
  echo "== AB8 release readiness =="
  check_group "AB8 Router Share acceptance" SERVER_URL CC_SWITCH_SERVER_TOKEN SHARE_ID CC_SWITCH_SHARE_URL
  check_group "AB8 Router Gateway/Share acceptance" ROUTER_API_TOKEN CC_SWITCH_SHARE_URL
  check_group "AB8 real provider tokens" CLAUDE_PROVIDER_TOKEN CODEX_PROVIDER_TOKEN GEMINI_PROVIDER_TOKEN
fi

echo "== summary =="
echo "blocked_groups=${FAILED_GROUPS}"
echo "external_blocked_groups=${EXTERNAL_BLOCKED_GROUPS}"

if [[ -n "$EVIDENCE_FILE" ]]; then
  BLOCKED_GROUPS="$FAILED_GROUPS" \
  EXTERNAL_BLOCKED_GROUPS="$EXTERNAL_BLOCKED_GROUPS" \
  GROK_GATE_STATUS="$(gate_status GROK_OAUTH_TEST_ACCOUNT CC_SWITCH_SHARE_URL ROUTER_API_TOKEN)" \
  QODER_GLOBAL_OAUTH_GATE_STATUS="$(gate_status QODER_GLOBAL_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_QODER_GLOBAL_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_OAUTH_CODEX_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_OAUTH_GEMINI_PROVIDER_ID)" \
  QODER_GLOBAL_PAT_GATE_STATUS="$(gate_status QODER_GLOBAL_PAT_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_QODER_GLOBAL_PAT_CLAUDE_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_PAT_CODEX_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_PAT_GEMINI_PROVIDER_ID)" \
  QODER_CN_OAUTH_GATE_STATUS="$(gate_status QODER_CN_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_QODER_CN_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_QODER_CN_OAUTH_CODEX_PROVIDER_ID CC_SWITCH_QODER_CN_OAUTH_GEMINI_PROVIDER_ID)" \
  COPILOT_GATE_STATUS="$(gate_status GITHUB_COPILOT_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID CC_SWITCH_COPILOT_CODEX_PROVIDER_ID CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID)" \
  AMAZON_Q_GATE_STATUS="$(gate_status AMAZON_Q_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_AMAZON_Q_CLAUDE_PROVIDER_ID CC_SWITCH_AMAZON_Q_CODEX_PROVIDER_ID)" \
  CODEX_IMAGES_GATE_STATUS="$([[ "${CC_SWITCH_CODEX_IMAGES_SMOKE:-0}" == "1" ]] && gate_status CC_SWITCH_SHARE_URL ROUTER_API_TOKEN || printf '%s' disabled)" \
  CLAUDE_MAX_5X_GATE_STATUS="$(gate_status CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT)" \
  CLAUDE_MAX_20X_GATE_STATUS="$(gate_status CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT)" \
  EVIDENCE_STAGE="${EVIDENCE_STAGE:-${STAGE}-env-check}" \
  EVIDENCE_STATUS="$([[ "$FAILED_GROUPS" -eq 0 ]] && echo ready || echo blocked)" \
    node scripts/smoke/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
fi

if [[ "$STRICT" == "1" && "$FAILED_GROUPS" -gt 0 ]]; then
  exit 2
fi
