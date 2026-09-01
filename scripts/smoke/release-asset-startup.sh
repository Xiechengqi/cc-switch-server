#!/usr/bin/env bash
set -euo pipefail
umask 077

BINARY="${1:?release binary path is required}"
ARCH="${2:-amd64}"
case "$ARCH" in
  amd64|arm64) ;;
  *) echo "unsupported release architecture: $ARCH" >&2; exit 2 ;;
esac
CONFIG_DIR="$(mktemp -d /tmp/cc-switch-release-asset.XXXXXX)"
LOG_FILE="${CONFIG_DIR}/server.log"
PORT="${PORT:-18121}"
PID=""

cleanup() {
  if [[ -n "$PID" ]]; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$CONFIG_DIR"
}
trap cleanup EXIT

run_binary() {
  "$BINARY" "$@"
}

run_binary --help >/dev/null
run_binary version --json | node -e '
let raw = "";
process.stdin.on("data", (chunk) => raw += chunk);
process.stdin.on("end", () => {
  const value = JSON.parse(raw);
  if (!/^[0-9a-f]{7,40}$/i.test(value.commitId || "")) process.exit(1);
});
'

# A typed Provider fixture forces registry lookup and runtime-plan compilation.
# It contains a non-secret test value and is removed with the temporary directory.
node - "$CONFIG_DIR/providers.json" <<'NODE'
const fs = require("fs");
const path = process.argv[2];
fs.writeFileSync(path, JSON.stringify({
  providers: [{
    app: "claude",
    provider: {
      id: "release-fixture-provider",
      name: "Anthropic API Key",
      settingsConfig: {
        apiKey: "release-fixture-non-secret",
        env: { ANTHROPIC_API_KEY: "release-fixture-non-secret" },
        modelMapping: { mode: "passthrough" }
      },
      meta: { providerType: "claude" }
    },
    providerType: "claude",
    providerTypeId: "claude",
    profileId: "claude.anthropic_api_key",
    profileSchemaRevision: 1,
    revision: 1,
    credentialGeneration: 1
  }]
}));
NODE

run_binary --config-dir "$CONFIG_DIR" doctor --startup-contracts-only >/dev/null

run_binary --host 127.0.0.1 --port "$PORT" --config-dir "$CONFIG_DIR" >"$LOG_FILE" 2>&1 &
PID=$!
for _ in $(seq 1 20); do
  if ! kill -0 "$PID" 2>/dev/null; then
    wait "$PID" || true
    echo "release asset exited during stateful startup" >&2
    tail -c 4096 "$LOG_FILE" >&2 || true
    exit 1
  fi
  if curl -fsS "http://127.0.0.1:${PORT}/version" >/dev/null 2>&1; then
    sleep 2
    kill -0 "$PID"
    exit 0
  fi
  sleep 1
done

echo "release asset did not pass the stateful startup health check" >&2
tail -c 4096 "$LOG_FILE" >&2 || true
exit 1
