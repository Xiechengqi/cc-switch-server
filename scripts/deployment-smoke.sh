#!/usr/bin/env bash
set -euo pipefail

MODE="${MODE:-binary}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-18083}"
SERVER_URL="http://${HOST}:${PORT}"
CONFIG_DIR="${CONFIG_DIR:-$(mktemp -d /tmp/cc-switch-server-deploy.XXXXXX)}"
KEEP_CONFIG_DIR="${KEEP_CONFIG_DIR:-0}"

cleanup() {
  case "${MODE}" in
    binary)
      if [[ -n "${PID:-}" ]]; then
        kill "$PID" 2>/dev/null || true
        wait "$PID" 2>/dev/null || true
      fi
      ;;
    docker)
      docker rm -f cc-switch-server-smoke >/dev/null 2>&1 || true
      ;;
  esac
  if [[ "$KEEP_CONFIG_DIR" != "1" ]]; then
    rm -rf "$CONFIG_DIR"
  else
    echo "kept config dir: $CONFIG_DIR"
  fi
}
trap cleanup EXIT

wait_health() {
  for _ in $(seq 1 40); do
    if curl -fsS "$SERVER_URL/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "server did not become healthy: $SERVER_URL" >&2
  return 1
}

setup_and_login() {
  echo "== setup =="
  curl -fsS -X POST \
    -H "Content-Type: application/json" \
    -d '{"password":"password123","ownerEmail":"owner@example.com","routerUrl":"http://127.0.0.1:9","clientTunnelSubdomain":"deploytest"}' \
    "$SERVER_URL/api/setup" >/dev/null

  echo "== login =="
  TOKEN="$(curl -fsS -X POST \
    -H "Content-Type: application/json" \
    -d '{"method":"password","password":"password123"}' \
    "$SERVER_URL/api/auth/login" | node -e 'let s="";process.stdin.on("data",d=>s+=d);process.stdin.on("end",()=>process.stdout.write(JSON.parse(s).token))')"
  export CC_SWITCH_SERVER_TOKEN="$TOKEN"
  echo "token length: ${#TOKEN}"
}

backup_restore_smoke() {
  echo "== backup restore smoke =="
  RESTORE=1 SERVER_URL="$SERVER_URL" CC_SWITCH_SERVER_TOKEN="$CC_SWITCH_SERVER_TOKEN" \
    scripts/backup-restore-smoke.sh
}

case "$MODE" in
  binary)
    cargo build
    target/debug/cc-switch-server --host "$HOST" --port "$PORT" --config-dir "$CONFIG_DIR" >"$CONFIG_DIR/server.log" 2>&1 &
    PID=$!
    wait_health
    curl -fsS "$SERVER_URL/version"
    echo
    setup_and_login
    backup_restore_smoke
    kill "$PID"
    wait "$PID" 2>/dev/null || true
    unset PID
    target/debug/cc-switch-server --host "$HOST" --port "$PORT" --config-dir "$CONFIG_DIR" >"$CONFIG_DIR/server-restart.log" 2>&1 &
    PID=$!
    wait_health
    echo "binary restart preserved config"
    ;;
  docker)
    if ! command -v docker >/dev/null 2>&1; then
      echo "docker is not installed" >&2
      exit 2
    fi
    docker build -t cc-switch-server:smoke .
    docker run -d --name cc-switch-server-smoke \
      -p "${PORT}:15721" \
      -v "${CONFIG_DIR}:/data/cc-switch-server" \
      cc-switch-server:smoke >/dev/null
    wait_health
    curl -fsS "$SERVER_URL/version"
    echo
    setup_and_login
    backup_restore_smoke
    docker restart cc-switch-server-smoke >/dev/null
    wait_health
    echo "docker restart preserved config"
    ;;
  systemd)
    cat <<'EOF'
systemd smoke requires a privileged target host.
Manual checklist:
1. cargo build --release
2. install target/release/cc-switch-server to /usr/local/bin/cc-switch-server
3. install deploy/cc-switch-server.service
4. systemctl daemon-reload && systemctl restart cc-switch-server
5. curl /health and /version
6. complete setup/login
7. run scripts/backup-restore-smoke.sh with SERVER_URL and CC_SWITCH_SERVER_TOKEN
8. systemctl restart cc-switch-server
9. confirm config, diagnostics and tunnel stopped/autoStart behavior
EOF
    ;;
  *)
    echo "unsupported MODE: $MODE (binary|docker|systemd)" >&2
    exit 2
    ;;
esac
