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

kiro_scope_prefix() {
  local auth_kind="$1"
  local region="$2"
  printf '%s_%s' "${auth_kind^^}" "${region^^}" | tr '-' '_'
}

kiro_gate_status() {
  local prefix
  prefix="$(kiro_scope_prefix "$1" "$2")"
  gate_status \
    SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN \
    CC_SWITCH_KIRO_SIGNED_USER CC_SWITCH_KIRO_SESSION_ID \
    "KIRO_${prefix}_TEST_ACCOUNT" \
    "CC_SWITCH_KIRO_${prefix}_CLAUDE_PROVIDER_ID" \
    "CC_SWITCH_KIRO_${prefix}_CODEX_PROVIDER_ID" \
    "CC_SWITCH_KIRO_${prefix}_SHARE_ID" \
    "CC_SWITCH_KIRO_${prefix}_MODEL" \
    "KIRO_${prefix}_REAL_RECEIPT_FILE"
}

check_kiro_receipt_scope() {
  local auth_kind="$1"
  local region="$2"
  local prefix
  prefix="$(kiro_scope_prefix "$auth_kind" "$region")"
  check_external_group \
    "AB7 Kiro ${auth_kind}/${region} receipt" \
    SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN \
    CC_SWITCH_KIRO_SIGNED_USER CC_SWITCH_KIRO_SESSION_ID \
    "KIRO_${prefix}_TEST_ACCOUNT" \
    "CC_SWITCH_KIRO_${prefix}_CLAUDE_PROVIDER_ID" \
    "CC_SWITCH_KIRO_${prefix}_CODEX_PROVIDER_ID" \
    "CC_SWITCH_KIRO_${prefix}_SHARE_ID" \
    "CC_SWITCH_KIRO_${prefix}_MODEL" \
    "KIRO_${prefix}_REAL_RECEIPT_FILE"
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
  echo "== AB5 Codex operation-scoped external gates =="
  check_external_group "AB5 GPT Image 2.5 receipt" CODEX_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CODEX_GPT_IMAGE_2_5_PROVIDER_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_SHARE_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_MODEL CODEX_GPT_IMAGE_2_5_REAL_RECEIPT_FILE
  check_external_group "AB5 GPT Image 2.5 Flare receipt" CODEX_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_PROVIDER_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_SHARE_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_MODEL CODEX_GPT_IMAGE_2_5_FLARE_REAL_RECEIPT_FILE
  check_external_group "AB5 GPT Image 2.5 Sunburst receipt" CODEX_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_PROVIDER_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_SHARE_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_MODEL CODEX_GPT_IMAGE_2_5_SUNBURST_REAL_RECEIPT_FILE
  check_external_group "AB5 Codex WS prewarm receipt" CODEX_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CODEX_WS_PREWARM_PROVIDER_ID CC_SWITCH_CODEX_WS_PREWARM_SHARE_ID CC_SWITCH_CODEX_WS_PREWARM_MODEL CODEX_WS_PREWARM_REAL_RECEIPT_FILE
  check_optional CODEX_REAL_RECEIPT_FILE
  echo "[INFO] Run node scripts/smoke/codex-real-receipt.mjs once per operation; input readiness is not live acceptance."
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
  echo "== AB6 Claude operation-scoped external gates =="
  check_external_group "AB6 Claude OAuth inference receipt" CLAUDE_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CLAUDE_OAUTH_PROVIDER_ID CC_SWITCH_CLAUDE_OAUTH_SHARE_ID CC_SWITCH_CLAUDE_OAUTH_MODEL CLAUDE_OAUTH_INFERENCE_REAL_RECEIPT_FILE
  check_external_group "AB6 Claude Max 5x plan receipt" CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CLAUDE_MAX_5X_PROVIDER_ID CC_SWITCH_CLAUDE_MAX_5X_SHARE_ID CC_SWITCH_CLAUDE_MAX_5X_MODEL CLAUDE_MAX_5X_REAL_RECEIPT_FILE
  check_external_group "AB6 Claude Max 20x plan receipt" CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CLAUDE_MAX_20X_PROVIDER_ID CC_SWITCH_CLAUDE_MAX_20X_SHARE_ID CC_SWITCH_CLAUDE_MAX_20X_MODEL CLAUDE_MAX_20X_REAL_RECEIPT_FILE
  check_external_group "AB6 Claude Fable 5.1 receipt" CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CLAUDE_FABLE_5_1_PROVIDER_ID CC_SWITCH_CLAUDE_FABLE_5_1_SHARE_ID CC_SWITCH_CLAUDE_FABLE_5_1_MODEL CLAUDE_FABLE_5_1_REAL_RECEIPT_FILE
  check_optional CLAUDE_REAL_RECEIPT_FILE
  echo "[INFO] Run node scripts/smoke/claude-real-receipt.mjs once per operation; input readiness is not live acceptance."
  check_group "AB6 Gemini OAuth real account" GEMINI_OAUTH_TEST_ACCOUNT GEMINI_OAUTH_CALLBACK_URL
  check_optional GEMINI_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional GEMINI_OAUTH_REFRESH_TOKEN
  check_optional GEMINI_CLI_CREDENTIALS_FIXTURE
  echo "== AB6 Antigravity/Agy independent external gates =="
  check_external_group "AB6 Antigravity OAuth two-surface receipt" ANTIGRAVITY_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_ANTIGRAVITY_OAUTH_SHARE_ID CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_PROVIDER_ID CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_MODEL CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_MODEL ANTIGRAVITY_OAUTH_REAL_RECEIPT_FILE
  check_external_group "AB6 Agy OAuth two-surface receipt" AGY_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_AGY_OAUTH_SHARE_ID CC_SWITCH_AGY_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_AGY_OAUTH_GEMINI_PROVIDER_ID CC_SWITCH_AGY_OAUTH_CLAUDE_MODEL CC_SWITCH_AGY_OAUTH_GEMINI_MODEL AGY_OAUTH_REAL_RECEIPT_FILE
  check_optional ANTIGRAVITY_OAUTH_CALLBACK_URL
  check_optional ANTIGRAVITY_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional ANTIGRAVITY_REAL_RECEIPT_FILE
  echo "[INFO] Run node scripts/smoke/antigravity-real.mjs once per rail; input readiness is not live acceptance."
  echo "== AB6 Grok OAuth operation-scoped external gates =="
  check_external_group "AB6 Grok inference receipt" GROK_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_GROK_INFERENCE_PROVIDER_ID CC_SWITCH_GROK_INFERENCE_SHARE_ID CC_SWITCH_GROK_INFERENCE_MODEL CC_SWITCH_GROK_SIGNED_USER CC_SWITCH_GROK_SESSION_ID CC_SWITCH_GROK_TURN_INDEX GROK_INFERENCE_REAL_RECEIPT_FILE
  check_external_group "AB6 Grok media receipt" GROK_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_GROK_MEDIA_PROVIDER_ID CC_SWITCH_GROK_MEDIA_SHARE_ID CC_SWITCH_GROK_MEDIA_MODEL CC_SWITCH_GROK_SIGNED_USER CC_SWITCH_GROK_SESSION_ID CC_SWITCH_GROK_TURN_INDEX GROK_MEDIA_REAL_RECEIPT_FILE
  check_external_group "AB6 Grok remote-compaction receipt" GROK_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_GROK_COMPACTION_PROVIDER_ID CC_SWITCH_GROK_COMPACTION_SHARE_ID CC_SWITCH_GROK_COMPACTION_MODEL CC_SWITCH_GROK_SIGNED_USER CC_SWITCH_GROK_SESSION_ID CC_SWITCH_GROK_TURN_INDEX GROK_REMOTE_COMPACTION_REAL_RECEIPT_FILE
  check_external_group "AB6 Grok normal-path probe" GROK_OAUTH_TEST_ACCOUNT CC_SWITCH_SHARE_URL ROUTER_API_TOKEN
  check_optional GROK_OAUTH_CALLBACK_URL
  check_optional GROK_OAUTH_REFRESH_TOKEN_FIXTURE
  check_optional GROK_OAUTH_AUTH_JSON_FIXTURE
  check_optional GROK_REAL_RECEIPT_FILE
  check_optional CC_SWITCH_GROK_MODEL
  check_optional CC_SWITCH_GROK_MEDIA_SMOKE
  echo "[INFO] Run node scripts/smoke/grok-real-receipt.mjs once per operation; the normal-path grok-oauth-real.mjs is probe_only/live_pending."
fi

if should_run "AB7"; then
  echo "== AB7 long-tail providers =="
  check_group "AB7 Cursor OAuth real account" CURSOR_OAUTH_TEST_ACCOUNT CURSOR_OAUTH_CALLBACK_URL
  check_any "AB7 Cursor credential fixture" CURSOR_OAUTH_REFRESH_TOKEN_FIXTURE CURSOR_API_KEY_FIXTURE
  echo "== AB7 Cursor independent rail receipts =="
  check_external_group "AB7 Cursor OAuth receipt" CURSOR_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CURSOR_OAUTH_PROVIDER_ID CC_SWITCH_CURSOR_OAUTH_SHARE_ID CC_SWITCH_CURSOR_OAUTH_MODEL CURSOR_OAUTH_REAL_RECEIPT_FILE
  check_external_group "AB7 Cursor API-key receipt" SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CURSOR_API_KEY_PROVIDER_ID CC_SWITCH_CURSOR_API_KEY_SHARE_ID CC_SWITCH_CURSOR_API_KEY_MODEL CURSOR_API_KEY_REAL_RECEIPT_FILE
  check_optional CURSOR_REAL_RECEIPT_FILE
  echo "[INFO] Run node scripts/smoke/cursor-real.mjs once per rail; input readiness is not live acceptance."
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
  echo "== AB7 Kiro auth-kind/region private receipt gates =="
  check_kiro_receipt_scope builder_id us-east-1
  check_kiro_receipt_scope builder_id eu-central-1
  check_kiro_receipt_scope idc us-east-1
  check_kiro_receipt_scope idc eu-central-1
  check_kiro_receipt_scope social us-east-1
  check_kiro_receipt_scope social eu-central-1
  check_kiro_receipt_scope api_key us-east-1
  check_kiro_receipt_scope api_key eu-central-1
  echo "[INFO] Run node scripts/smoke/kiro-real-receipt.mjs once per auth kind and region; input readiness is not live acceptance."
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
  GROK_INFERENCE_GATE_STATUS="$(gate_status GROK_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_GROK_INFERENCE_PROVIDER_ID CC_SWITCH_GROK_INFERENCE_SHARE_ID CC_SWITCH_GROK_INFERENCE_MODEL CC_SWITCH_GROK_SIGNED_USER CC_SWITCH_GROK_SESSION_ID CC_SWITCH_GROK_TURN_INDEX GROK_INFERENCE_REAL_RECEIPT_FILE)" \
  GROK_MEDIA_GATE_STATUS="$(gate_status GROK_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_GROK_MEDIA_PROVIDER_ID CC_SWITCH_GROK_MEDIA_SHARE_ID CC_SWITCH_GROK_MEDIA_MODEL CC_SWITCH_GROK_SIGNED_USER CC_SWITCH_GROK_SESSION_ID CC_SWITCH_GROK_TURN_INDEX GROK_MEDIA_REAL_RECEIPT_FILE)" \
  GROK_REMOTE_COMPACTION_GATE_STATUS="$(gate_status GROK_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_GROK_COMPACTION_PROVIDER_ID CC_SWITCH_GROK_COMPACTION_SHARE_ID CC_SWITCH_GROK_COMPACTION_MODEL CC_SWITCH_GROK_SIGNED_USER CC_SWITCH_GROK_SESSION_ID CC_SWITCH_GROK_TURN_INDEX GROK_REMOTE_COMPACTION_REAL_RECEIPT_FILE)" \
  CLAUDE_OAUTH_INFERENCE_GATE_STATUS="$(gate_status CLAUDE_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CLAUDE_OAUTH_PROVIDER_ID CC_SWITCH_CLAUDE_OAUTH_SHARE_ID CC_SWITCH_CLAUDE_OAUTH_MODEL CLAUDE_OAUTH_INFERENCE_REAL_RECEIPT_FILE)" \
  CLAUDE_MAX_5X_GATE_STATUS="$(gate_status CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CLAUDE_MAX_5X_PROVIDER_ID CC_SWITCH_CLAUDE_MAX_5X_SHARE_ID CC_SWITCH_CLAUDE_MAX_5X_MODEL CLAUDE_MAX_5X_REAL_RECEIPT_FILE)" \
  CLAUDE_MAX_20X_GATE_STATUS="$(gate_status CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CLAUDE_MAX_20X_PROVIDER_ID CC_SWITCH_CLAUDE_MAX_20X_SHARE_ID CC_SWITCH_CLAUDE_MAX_20X_MODEL CLAUDE_MAX_20X_REAL_RECEIPT_FILE)" \
  CLAUDE_FABLE_5_1_GATE_STATUS="$(gate_status CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CLAUDE_FABLE_5_1_PROVIDER_ID CC_SWITCH_CLAUDE_FABLE_5_1_SHARE_ID CC_SWITCH_CLAUDE_FABLE_5_1_MODEL CLAUDE_FABLE_5_1_REAL_RECEIPT_FILE)" \
  CODEX_GPT_IMAGE_2_5_GATE_STATUS="$(gate_status CODEX_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CODEX_GPT_IMAGE_2_5_PROVIDER_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_SHARE_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_MODEL CODEX_GPT_IMAGE_2_5_REAL_RECEIPT_FILE)" \
  CODEX_GPT_IMAGE_2_5_FLARE_GATE_STATUS="$(gate_status CODEX_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_PROVIDER_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_SHARE_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_FLARE_MODEL CODEX_GPT_IMAGE_2_5_FLARE_REAL_RECEIPT_FILE)" \
  CODEX_GPT_IMAGE_2_5_SUNBURST_GATE_STATUS="$(gate_status CODEX_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_PROVIDER_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_SHARE_ID CC_SWITCH_CODEX_GPT_IMAGE_2_5_SUNBURST_MODEL CODEX_GPT_IMAGE_2_5_SUNBURST_REAL_RECEIPT_FILE)" \
  CODEX_WS_PREWARM_GATE_STATUS="$(gate_status CODEX_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CODEX_WS_PREWARM_PROVIDER_ID CC_SWITCH_CODEX_WS_PREWARM_SHARE_ID CC_SWITCH_CODEX_WS_PREWARM_MODEL CODEX_WS_PREWARM_REAL_RECEIPT_FILE)" \
  ANTIGRAVITY_OAUTH_GATE_STATUS="$(gate_status ANTIGRAVITY_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_ANTIGRAVITY_OAUTH_SHARE_ID CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_PROVIDER_ID CC_SWITCH_ANTIGRAVITY_OAUTH_CLAUDE_MODEL CC_SWITCH_ANTIGRAVITY_OAUTH_GEMINI_MODEL ANTIGRAVITY_OAUTH_REAL_RECEIPT_FILE)" \
  AGY_OAUTH_GATE_STATUS="$(gate_status AGY_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_AGY_OAUTH_SHARE_ID CC_SWITCH_AGY_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_AGY_OAUTH_GEMINI_PROVIDER_ID CC_SWITCH_AGY_OAUTH_CLAUDE_MODEL CC_SWITCH_AGY_OAUTH_GEMINI_MODEL AGY_OAUTH_REAL_RECEIPT_FILE)" \
  CURSOR_OAUTH_GATE_STATUS="$(gate_status CURSOR_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CURSOR_OAUTH_PROVIDER_ID CC_SWITCH_CURSOR_OAUTH_SHARE_ID CC_SWITCH_CURSOR_OAUTH_MODEL CURSOR_OAUTH_REAL_RECEIPT_FILE)" \
  CURSOR_API_KEY_GATE_STATUS="$(gate_status SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_CURSOR_API_KEY_PROVIDER_ID CC_SWITCH_CURSOR_API_KEY_SHARE_ID CC_SWITCH_CURSOR_API_KEY_MODEL CURSOR_API_KEY_REAL_RECEIPT_FILE)" \
  KIRO_BUILDER_ID_US_EAST_1_GATE_STATUS="$(kiro_gate_status builder_id us-east-1)" \
  KIRO_BUILDER_ID_EU_CENTRAL_1_GATE_STATUS="$(kiro_gate_status builder_id eu-central-1)" \
  KIRO_IDC_US_EAST_1_GATE_STATUS="$(kiro_gate_status idc us-east-1)" \
  KIRO_IDC_EU_CENTRAL_1_GATE_STATUS="$(kiro_gate_status idc eu-central-1)" \
  KIRO_SOCIAL_US_EAST_1_GATE_STATUS="$(kiro_gate_status social us-east-1)" \
  KIRO_SOCIAL_EU_CENTRAL_1_GATE_STATUS="$(kiro_gate_status social eu-central-1)" \
  KIRO_API_KEY_US_EAST_1_GATE_STATUS="$(kiro_gate_status api_key us-east-1)" \
  KIRO_API_KEY_EU_CENTRAL_1_GATE_STATUS="$(kiro_gate_status api_key eu-central-1)" \
  QODER_GLOBAL_OAUTH_GATE_STATUS="$(gate_status QODER_GLOBAL_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_QODER_GLOBAL_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_OAUTH_CODEX_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_OAUTH_GEMINI_PROVIDER_ID)" \
  QODER_GLOBAL_PAT_GATE_STATUS="$(gate_status QODER_GLOBAL_PAT_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_QODER_GLOBAL_PAT_CLAUDE_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_PAT_CODEX_PROVIDER_ID CC_SWITCH_QODER_GLOBAL_PAT_GEMINI_PROVIDER_ID)" \
  QODER_CN_OAUTH_GATE_STATUS="$(gate_status QODER_CN_OAUTH_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_QODER_CN_OAUTH_CLAUDE_PROVIDER_ID CC_SWITCH_QODER_CN_OAUTH_CODEX_PROVIDER_ID CC_SWITCH_QODER_CN_OAUTH_GEMINI_PROVIDER_ID)" \
  COPILOT_GATE_STATUS="$(gate_status GITHUB_COPILOT_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID CC_SWITCH_COPILOT_CODEX_PROVIDER_ID CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID)" \
  AMAZON_Q_GATE_STATUS="$(gate_status AMAZON_Q_TEST_ACCOUNT SERVER_URL CC_SWITCH_SERVER_TOKEN CC_SWITCH_SHARE_URL ROUTER_API_TOKEN CC_SWITCH_AMAZON_Q_CLAUDE_PROVIDER_ID CC_SWITCH_AMAZON_Q_CODEX_PROVIDER_ID)" \
  CODEX_IMAGES_GATE_STATUS="$([[ "${CC_SWITCH_CODEX_IMAGES_SMOKE:-0}" == "1" ]] && gate_status CC_SWITCH_SHARE_URL ROUTER_API_TOKEN || printf '%s' disabled)" \
  EVIDENCE_STAGE="${EVIDENCE_STAGE:-${STAGE}-env-check}" \
  EVIDENCE_STATUS="$([[ "$FAILED_GROUPS" -eq 0 ]] && echo ready || echo blocked)" \
    node scripts/smoke/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
fi

if [[ "$STRICT" == "1" && "$FAILED_GROUPS" -gt 0 ]]; then
  exit 2
fi
