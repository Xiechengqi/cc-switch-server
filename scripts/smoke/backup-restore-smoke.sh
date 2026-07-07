#!/usr/bin/env bash
set -euo pipefail

SERVER_URL="${SERVER_URL:-http://127.0.0.1:15721}"
SERVER_TOKEN="${SERVER_TOKEN:-${CC_SWITCH_SERVER_TOKEN:-}}"
RESTORE="${RESTORE:-0}"

if [[ -z "$SERVER_TOKEN" ]]; then
  echo "SERVER_TOKEN or CC_SWITCH_SERVER_TOKEN is required" >&2
  exit 2
fi

auth_header=(-H "Authorization: Bearer $SERVER_TOKEN")
json_header=(-H "Content-Type: application/json")

echo "== create backup =="
create_response="$(
  curl -sS -X POST "$SERVER_URL/api/backup" \
    "${auth_header[@]}" "${json_header[@]}" \
    -d '{"reason":"smoke"}'
)"
echo "$create_response"

backup_id="$(
  CREATE_RESPONSE="$create_response" node - <<'NODE'
const response = JSON.parse(process.env.CREATE_RESPONSE || "{}");
const id = response.backup && response.backup.id;
if (!id) process.exit(1);
process.stdout.write(id);
NODE
)"

echo
echo "== list backups =="
curl -sS "$SERVER_URL/api/backup" "${auth_header[@]}"
echo

if [[ "$RESTORE" == "1" ]]; then
  echo "== restore backup $backup_id =="
  curl -sS -X POST "$SERVER_URL/api/backup/$backup_id/restore" "${auth_header[@]}"
  echo
else
  echo "restore skipped; set RESTORE=1 to restore $backup_id"
fi
