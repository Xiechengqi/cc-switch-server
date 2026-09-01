#!/usr/bin/env bash
set -euo pipefail

# Keep the release gate on the same test stack used by the repository's full
# test instructions; otherwise protocol fixtures can abort with SIGABRT before
# the gate reports a meaningful result.
export RUST_MIN_STACK="${RUST_MIN_STACK:-67108864}"

RUN_TESTS="${RUN_TESTS:-1}"
RUN_REAL="${RUN_REAL:-0}"
RUN_DEPLOYMENT_TESTS="${RUN_DEPLOYMENT_TESTS:-0}"
EVIDENCE_FILE="${EVIDENCE_FILE:-}"
FAILURES=0
WARNINGS=0
BLOCKERS=()
INTERNAL_BLOCKERS=()
DEPLOYMENT_NOT_TESTED=false
CODEX_IMAGES_GATE_STATUS=disabled

pass() { echo "[PASS] $*"; }
warn() { WARNINGS=$((WARNINGS + 1)); echo "[WARN] $*"; }
fail() { FAILURES=$((FAILURES + 1)); echo "[FAIL] $*"; }
block() { BLOCKERS+=("$*"); echo "[BLOCKED] $*"; }
internal_block() { INTERNAL_BLOCKERS+=("$*"); echo "[BLOCKED-INTERNAL] $*"; }

need_var() {
  local name="$1"
  if [[ -z "${!name:-}" || "${!name}" == \<* ]]; then
    block "$name"
  fi
}

echo "== local release checks =="
node scripts/audit/audit-proxy-bridge-contract.mjs --check || FAILURES=$((FAILURES + 1))
node scripts/audit/audit-token-market-decoupling.mjs --check || FAILURES=$((FAILURES + 1))
if [[ "$RUN_TESTS" == "1" ]]; then
  LOCAL_FAILURES_BEFORE="$FAILURES"
  cargo fmt --check || FAILURES=$((FAILURES + 1))
  cargo test || FAILURES=$((FAILURES + 1))
  scripts/audit/validate-local.sh || FAILURES=$((FAILURES + 1))
  scripts/smoke/smoke-local.sh || FAILURES=$((FAILURES + 1))
  if [[ "$FAILURES" -eq "$LOCAL_FAILURES_BEFORE" ]]; then
    pass "local test suite executed"
  else
    echo "[FAIL] local test suite completed with failures"
  fi
else
  warn "RUN_TESTS=0; local test suite skipped"
  internal_block "local-contracts-unverified"
fi

echo "== AB env gates =="
need_var CC_SWITCH_SERVER_TOKEN
need_var SHARE_ID
need_var CC_SWITCH_SHARE_URL
need_var ROUTER_BASE_URL
need_var ROUTER_API_TOKEN
need_var CLAUDE_PROVIDER_TOKEN
need_var CODEX_PROVIDER_TOKEN
need_var GEMINI_PROVIDER_TOKEN
case "${CC_SWITCH_CODEX_IMAGES_SMOKE:-0}" in
  0) ;;
  1)
    CODEX_IMAGES_GATE_STATUS=configured-not-run
    for name in CC_SWITCH_SHARE_URL ROUTER_API_TOKEN; do
      if [[ -z "${!name:-}" || "${!name}" == \<* ]]; then
        CODEX_IMAGES_GATE_STATUS=blocked-inputs
      fi
      need_var "$name"
    done
    ;;
  *)
    CODEX_IMAGES_GATE_STATUS=invalid-configuration
    fail "CC_SWITCH_CODEX_IMAGES_SMOKE must be 0 or 1"
    ;;
esac

if [[ "$RUN_REAL" == "1" && "${#BLOCKERS[@]}" -eq 0 ]]; then
  echo "== real smoke =="
  scripts/smoke/router-share-smoke.sh || FAILURES=$((FAILURES + 1))
  scripts/smoke/code-agent-regression.sh || FAILURES=$((FAILURES + 1))
  if [[ "${CC_SWITCH_CODEX_IMAGES_SMOKE:-0}" == "1" ]]; then
    if node scripts/smoke/codex-images-real.mjs; then
      CODEX_IMAGES_GATE_STATUS=passed
    else
      CODEX_IMAGES_GATE_STATUS=failed
      FAILURES=$((FAILURES + 1))
    fi
  fi
else
  warn "real smoke skipped; RUN_REAL=${RUN_REAL}, blockers=${#BLOCKERS[@]}"
fi

echo "== deployment boundary =="
if [[ "$RUN_DEPLOYMENT_TESTS" == "1" ]]; then
  scripts/smoke/deployment-smoke.sh || FAILURES=$((FAILURES + 1))
  pass "deployment smoke executed"
else
  DEPLOYMENT_NOT_TESTED=true
  block "deployment-not-tested"
fi

echo "== secret audit =="
secret_audit_files=(
  docs/provider/regression-matrix.json
  assets/contract/provider-legacy-compatibility.json
)
while IFS= read -r file; do
  secret_audit_files+=("$file")
done < <(find docs/provider-fixtures -type f -name '*.json' | sort)
scripts/audit/evidence-redaction-check.sh "${secret_audit_files[@]}" || FAILURES=$((FAILURES + 1))

if [[ "$FAILURES" -gt 0 ]]; then
  RELEASE_DECISION="not-ready"
  EVIDENCE_VERIFICATION_STATE="failed"
elif [[ "${#INTERNAL_BLOCKERS[@]}" -gt 0 ]]; then
  RELEASE_DECISION="blocked"
  EVIDENCE_VERIFICATION_STATE="blocked_inputs"
elif [[ "${#BLOCKERS[@]}" -gt 0 ]]; then
  RELEASE_DECISION="ready-with-known-external-blockers"
  EVIDENCE_VERIFICATION_STATE="contract_verified"
else
  RELEASE_DECISION="ready"
  EVIDENCE_VERIFICATION_STATE="contract_verified"
fi

echo "== release decision =="
echo "decision=${RELEASE_DECISION}"
echo "verificationState=${EVIDENCE_VERIFICATION_STATE}"
echo "failures=${FAILURES} warnings=${WARNINGS} blockers=${#BLOCKERS[@]} internalBlockers=${#INTERNAL_BLOCKERS[@]}"
if [[ "${#INTERNAL_BLOCKERS[@]}" -gt 0 ]]; then
  printf 'internal blockers:\n'
  printf '  - %s\n' "${INTERNAL_BLOCKERS[@]}"
fi
if [[ "${#BLOCKERS[@]}" -gt 0 ]]; then
  printf 'blockers:\n'
  printf '  - %s\n' "${BLOCKERS[@]}"
fi

if [[ -n "$EVIDENCE_FILE" ]]; then
  EVIDENCE_STAGE="${EVIDENCE_STAGE:-AB8-release-readiness}" \
  EVIDENCE_TARGET="${EVIDENCE_TARGET:-release-readiness}" \
  EVIDENCE_STATUS="$RELEASE_DECISION" \
  EVIDENCE_VERIFICATION_STATE="$EVIDENCE_VERIFICATION_STATE" \
  EVIDENCE_VERIFICATION_SCOPE="local-contracts-and-configured-release-gates" \
  CODEX_IMAGES_GATE_STATUS="$CODEX_IMAGES_GATE_STATUS" \
  RELEASE_DECISION="$RELEASE_DECISION" \
  DEPLOYMENT_NOT_TESTED="$DEPLOYMENT_NOT_TESTED" \
  FAILURES="$FAILURES" WARNINGS="$WARNINGS" \
  BLOCKER_GROUP="$([[ "${#INTERNAL_BLOCKERS[@]}" -gt 0 ]] && echo local-contracts-unverified || ([[ "$DEPLOYMENT_NOT_TESTED" == "true" ]] && echo deployment-not-tested || echo external-readonly))" \
  BLOCKED_GROUPS="${INTERNAL_BLOCKERS[*]}" \
  EXTERNAL_BLOCKED_GROUPS="${BLOCKERS[*]}" \
  FAILURE_CLASS="$([[ "$FAILURES" -gt 0 ]] && echo release-gate || echo "")" \
  EVIDENCE_NOTES="internalBlockers=${INTERNAL_BLOCKERS[*]:-none}; externalBlockers=${BLOCKERS[*]:-none}" \
    node scripts/smoke/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
  scripts/audit/evidence-redaction-check.sh "$EVIDENCE_FILE"
fi

if [[ "$RELEASE_DECISION" == "not-ready" || "$RELEASE_DECISION" == "blocked" ]]; then
  exit 1
fi
