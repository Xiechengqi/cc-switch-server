#!/usr/bin/env bash
set -euo pipefail

RUN_TESTS="${RUN_TESTS:-1}"
RUN_REAL="${RUN_REAL:-0}"
RUN_DEPLOYMENT_TESTS="${RUN_DEPLOYMENT_TESTS:-0}"
EVIDENCE_FILE="${EVIDENCE_FILE:-}"
FAILURES=0
WARNINGS=0
BLOCKERS=()
DEPLOYMENT_NOT_TESTED=false

pass() { echo "[PASS] $*"; }
warn() { WARNINGS=$((WARNINGS + 1)); echo "[WARN] $*"; }
fail() { FAILURES=$((FAILURES + 1)); echo "[FAIL] $*"; }
block() { BLOCKERS+=("$*"); echo "[BLOCKED] $*"; }

need_var() {
  local name="$1"
  if [[ -z "${!name:-}" || "${!name}" == \<* ]]; then
    block "$name"
  fi
}

echo "== local release checks =="
if [[ "$RUN_TESTS" == "1" ]]; then
  cargo fmt --check || FAILURES=$((FAILURES + 1))
  cargo test || FAILURES=$((FAILURES + 1))
  scripts/audit/validate-local.sh || FAILURES=$((FAILURES + 1))
  scripts/smoke/smoke-local.sh || FAILURES=$((FAILURES + 1))
  pass "local test suite executed"
else
  warn "RUN_TESTS=0; local test suite skipped"
fi

echo "== AB env gates =="
need_var CC_SWITCH_SERVER_TOKEN
need_var SHARE_ID
need_var DIRECT_SHARE_URL
need_var ROUTER_API_TOKEN
need_var MARKET_API_URL
need_var CLAUDE_PROVIDER_TOKEN
need_var CODEX_PROVIDER_TOKEN
need_var GEMINI_PROVIDER_TOKEN
need_var SHARE_MARKET_URL
need_var SHARE_MARKET_GRANT_TOKEN
need_var SHARE_MARKET_BUYER_EMAIL
need_var SHARE_MARKET_LISTING_ID
need_var SHARE_MARKET_ORDER_ID

if [[ "$RUN_REAL" == "1" && "${#BLOCKERS[@]}" -eq 0 ]]; then
  echo "== real smoke =="
  scripts/smoke/router-market-smoke.sh || FAILURES=$((FAILURES + 1))
  scripts/smoke/code-agent-regression.sh || FAILURES=$((FAILURES + 1))
  scripts/smoke/share-market-grant-smoke.sh || FAILURES=$((FAILURES + 1))
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
  docs/code-agent-regression-matrix.json
  assets/contract/provider-fixtures/structures.json
)
while IFS= read -r file; do
  secret_audit_files+=("$file")
done < <(find docs/provider-fixtures -type f -name '*.json' | sort)
scripts/audit/evidence-redaction-check.sh "${secret_audit_files[@]}" || FAILURES=$((FAILURES + 1))

if [[ "$FAILURES" -gt 0 ]]; then
  RELEASE_DECISION="not-ready"
elif [[ "${#BLOCKERS[@]}" -gt 0 ]]; then
  RELEASE_DECISION="ready-with-known-external-blockers"
else
  RELEASE_DECISION="ready"
fi

echo "== release decision =="
echo "decision=${RELEASE_DECISION}"
echo "failures=${FAILURES} warnings=${WARNINGS} blockers=${#BLOCKERS[@]}"
if [[ "${#BLOCKERS[@]}" -gt 0 ]]; then
  printf 'blockers:\n'
  printf '  - %s\n' "${BLOCKERS[@]}"
fi

if [[ -n "$EVIDENCE_FILE" ]]; then
  EVIDENCE_STAGE="${EVIDENCE_STAGE:-AB8-release-readiness}" \
  EVIDENCE_TARGET="${EVIDENCE_TARGET:-release-readiness}" \
  EVIDENCE_STATUS="$RELEASE_DECISION" \
  RELEASE_DECISION="$RELEASE_DECISION" \
  DEPLOYMENT_NOT_TESTED="$DEPLOYMENT_NOT_TESTED" \
  FAILURES="$FAILURES" WARNINGS="$WARNINGS" \
  BLOCKER_GROUP="$([[ "$DEPLOYMENT_NOT_TESTED" == "true" ]] && echo deployment-not-tested || echo external-readonly)" \
  FAILURE_CLASS="$([[ "$FAILURES" -gt 0 ]] && echo release-gate || echo "")" \
  EVIDENCE_NOTES="blockers=${BLOCKERS[*]:-none}" \
    node scripts/smoke/write-acceptance-evidence.mjs --out "$EVIDENCE_FILE"
  scripts/audit/evidence-redaction-check.sh "$EVIDENCE_FILE"
fi

if [[ "$RELEASE_DECISION" == "not-ready" ]]; then
  exit 1
fi
