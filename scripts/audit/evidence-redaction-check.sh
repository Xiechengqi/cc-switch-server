#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -lt 1 ]]; then
  echo "usage: scripts/audit/evidence-redaction-check.sh <file> [file...]" >&2
  exit 2
fi

STATUS=0
for file in "$@"; do
  if [[ ! -f "$file" ]]; then
    echo "[FAIL] evidence file not found: $file" >&2
    STATUS=1
    continue
  fi

  if grep -Eiq 'Bearer[[:space:]]+[A-Za-z0-9._~+/=-]{10,}|sk-[A-Za-z0-9._-]{10,}|ya29\.[A-Za-z0-9._-]+|refresh[_-]?token["'\'']?[[:space:]]*[:=][[:space:]]*["'\''][^"'\'']{6,}|access[_-]?token["'\'']?[[:space:]]*[:=][[:space:]]*["'\''][^"'\'']{6,}' "$file"; then
    echo "[FAIL] secret-like content detected: $file" >&2
    STATUS=1
  else
    echo "[PASS] redaction check: $file"
  fi
done

exit "$STATUS"
